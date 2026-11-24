//! Which panels show: those of the configured dashboard, or the manual ones.

use crate::configuration::{Configuration, Imported, Panel};
use crate::error::Error;
use std::vec::Vec;

/// Outcome of a dashboard import at start or on request.
#[derive(Debug, Clone, PartialEq)]
pub enum Reload {
    /// The dashboard was imported and cached.
    Imported,
    /// The import failed; the cached panels, if any, still show.
    Failed(Error),
}

impl Configuration {
    /// The cached import, if it belongs to the configured dashboard.
    pub fn current_import(&self) -> Option<&Imported> {
        let uid = self.dashboard.as_deref()?;
        self.imported
            .as_ref()
            .filter(|imported| imported.uid == uid)
    }

    /// Panels to show, in order: the dashboard's, except the hidden ones,
    /// when a dashboard is set, else the manual panels.
    pub fn visible_panels(&self) -> Vec<&Panel> {
        match &self.dashboard {
            Some(_) => self
                .current_import()
                .map(|imported| {
                    imported
                        .panels
                        .iter()
                        .filter(|panel| !self.is_hidden(panel))
                        .collect()
                })
                .unwrap_or_default(),
            None => self.panels.iter().collect(),
        }
    }

    /// Records the result of importing the configured dashboard. A success
    /// replaces the cache; hidden keys are kept, so panels hidden before stay
    /// hidden. A failure keeps the previous cache.
    pub fn apply_import(&mut self, result: Result<Imported, Error>) -> Reload {
        match result {
            Ok(imported) => {
                self.imported = Some(imported);
                Reload::Imported
            }
            Err(error) => Reload::Failed(error),
        }
    }

    /// Copies the visible dashboard panels into the manual list, and stops
    /// following the dashboard.
    pub fn convert_to_manual(&mut self) {
        let panels = self
            .visible_panels()
            .into_iter()
            .cloned()
            .map(|mut panel| {
                panel.key = None;
                panel
            })
            .collect();
        self.panels = panels;
        self.dashboard = None;
        self.imported = None;
        self.hidden.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::Kind;
    use std::string::String;

    fn panel(key: &str, title: &str) -> Panel {
        Panel {
            key: Some(String::from(key)),
            title: String::from(title),
            expression: std::format!("metric_{key}"),
            ..Panel::new()
        }
    }

    fn import(panels: Vec<Panel>) -> Imported {
        Imported {
            uid: String::from("servers"),
            title: String::from("Servers"),
            skipped: 0,
            panels,
        }
    }

    fn configuration() -> Configuration {
        Configuration {
            url: String::from("https://grafana.example.com"),
            data_source: String::from("prometheus"),
            refresh_seconds: 60,
            dashboard: Some(String::from("servers")),
            ..Configuration::default()
        }
    }

    fn titles(configuration: &Configuration) -> Vec<&str> {
        configuration
            .visible_panels()
            .iter()
            .map(|panel| panel.title.as_str())
            .collect()
    }

    #[test]
    fn failed_import_falls_back_to_the_cache() {
        let mut configuration = configuration();
        assert!(titles(&configuration).is_empty());
        assert_eq!(
            configuration.apply_import(Ok(import(std::vec![
                panel("1", "CPU"),
                panel("2", "Memory")
            ]))),
            Reload::Imported
        );
        assert_eq!(titles(&configuration), ["CPU", "Memory"]);
        // Offline at the next start: the cached panels still show.
        assert_eq!(
            configuration.apply_import(Err(Error::Network)),
            Reload::Failed(Error::Network)
        );
        assert_eq!(titles(&configuration), ["CPU", "Memory"]);
        assert_eq!(
            configuration.apply_import(Err(Error::Unauthorized)),
            Reload::Failed(Error::Unauthorized)
        );
        assert_eq!(titles(&configuration), ["CPU", "Memory"]);
        // The cache survives a save and a load.
        let reloaded = Configuration::parse(&configuration.to_json()).unwrap();
        assert_eq!(titles(&reloaded), ["CPU", "Memory"]);
        // A cache from another dashboard is not used.
        configuration.dashboard = Some(String::from("other"));
        assert!(titles(&configuration).is_empty());
    }

    #[test]
    fn hidden_panels_stay_hidden_after_a_reload() {
        let mut configuration = configuration();
        configuration.apply_import(Ok(import(std::vec![
            panel("1", "CPU"),
            panel("2", "Memory")
        ])));
        configuration.set_hidden("2", true);
        assert_eq!(titles(&configuration), ["CPU"]);
        // The dashboard changed: Memory was renamed and a panel was added.
        configuration.apply_import(Ok(import(std::vec![
            panel("1", "CPU"),
            panel("2", "Memory used"),
            panel("3", "Disk"),
        ])));
        assert_eq!(titles(&configuration), ["CPU", "Disk"]);
        let reloaded = Configuration::parse(&configuration.to_json()).unwrap();
        assert_eq!(titles(&reloaded), ["CPU", "Disk"]);
    }

    #[test]
    fn manual_panels_show_without_a_dashboard() {
        let mut configuration = configuration();
        configuration.apply_import(Ok(import(std::vec![
            panel("1", "CPU"),
            panel("2", "Memory")
        ])));
        configuration.set_hidden("1", true);
        configuration.convert_to_manual();
        assert_eq!(configuration.dashboard, None);
        assert_eq!(configuration.imported, None);
        assert!(configuration.hidden.is_empty());
        assert_eq!(titles(&configuration), ["Memory"]);
        assert_eq!(configuration.panels[0].key, None);
        assert_eq!(configuration.panels[0].kind(), Kind::Value);
    }
}
