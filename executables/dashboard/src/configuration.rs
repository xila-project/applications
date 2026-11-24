//! The configuration file: connection, panels and the cached dashboard.

use crate::error::{Error, Result};
use crate::format::{MAXIMUM_DECIMALS, Unit};
use miniserde::{Deserialize, Serialize, json};
use std::{string::String, vec::Vec};

/// Refresh period used when the configuration does not set one.
pub const DEFAULT_REFRESH_SECONDS: u32 = 60;
/// Shortest refresh period accepted.
pub const MINIMUM_REFRESH_SECONDS: u32 = 10;
/// Chart range used when a panel does not set one.
pub const DEFAULT_RANGE_SECONDS: u32 = 3_600;
/// Longest chart range: an imported dashboard's range is capped to it.
pub const MAXIMUM_RANGE_SECONDS: u32 = 86_400;
/// Chart ranges offered in the panel editor.
pub const RANGES: [u32; 5] = [900, 3_600, 6 * 3_600, 12 * 3_600, 86_400];
/// Most panels kept, from a dashboard or written by hand.
pub const MAXIMUM_PANELS: usize = 48;
/// Longest PromQL expression kept.
pub const MAXIMUM_EXPRESSION_LENGTH: usize = 2_048;
/// Longest title kept.
pub const MAXIMUM_TITLE_LENGTH: usize = 64;

/// How a panel is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Kind {
    /// The last value in big text (an instant query).
    #[default]
    #[serde(rename = "value")]
    Value,
    /// The last value and a small line chart (a range query).
    #[serde(rename = "chart")]
    Chart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Panel {
    /// Stable key of an imported panel (its Grafana id), used to hide it.
    pub key: Option<String>,
    pub title: String,
    /// PromQL expression, run through Grafana.
    pub expression: String,
    pub unit: Option<Unit>,
    /// Fixed number of decimals; automatic when absent.
    pub decimals: Option<u8>,
    pub warning: Option<f64>,
    pub critical: Option<f64>,
    pub kind: Option<Kind>,
    /// Chart range in seconds.
    pub range_seconds: Option<u32>,
}

impl Panel {
    pub fn new() -> Self {
        Self {
            key: None,
            title: String::new(),
            expression: String::new(),
            unit: None,
            decimals: None,
            warning: None,
            critical: None,
            kind: None,
            range_seconds: None,
        }
    }

    pub fn unit(&self) -> Unit {
        self.unit.unwrap_or_default()
    }

    pub fn kind(&self) -> Kind {
        self.kind.unwrap_or_default()
    }

    pub fn range_seconds(&self) -> u32 {
        self.range_seconds
            .unwrap_or(DEFAULT_RANGE_SECONDS)
            .clamp(60, MAXIMUM_RANGE_SECONDS)
    }

    /// Key used to hide the panel: its Grafana id, else its title.
    pub fn hiding_key(&self) -> &str {
        self.key.as_deref().unwrap_or(&self.title)
    }

    pub fn validate(&self) -> core::result::Result<(), Invalid> {
        if self.title.trim().is_empty() || self.title.len() > MAXIMUM_TITLE_LENGTH {
            return Err(Invalid::Title);
        }
        if self.expression.trim().is_empty() || self.expression.len() > MAXIMUM_EXPRESSION_LENGTH {
            return Err(Invalid::Expression);
        }
        if self
            .decimals
            .is_some_and(|decimals| decimals > MAXIMUM_DECIMALS)
        {
            return Err(Invalid::Decimals);
        }
        if [self.warning, self.critical]
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(Invalid::Threshold);
        }
        Ok(())
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

/// The last dashboard imported, kept so that the panels still show when the
/// dashboard cannot be fetched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    /// Uid of the dashboard these panels come from.
    pub uid: String,
    pub title: String,
    /// Panels of a kind the application cannot show.
    pub skipped: u32,
    pub panels: Vec<Panel>,
}

/// What the user can get wrong in the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    Url,
    DataSource,
    Refresh,
    Title,
    Expression,
    Decimals,
    Threshold,
    Dashboard,
    TooManyPanels,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Configuration {
    /// Grafana base URL, such as `https://grafana.example.com`.
    pub url: String,
    /// Uid of the Prometheus-compatible data source the queries run on.
    pub data_source: String,
    pub refresh_seconds: u32,
    /// Uid of the dashboard to import at start, if any.
    pub dashboard: Option<String>,
    /// Keys of the imported panels the user hid.
    pub hidden: Vec<String>,
    /// Panels written by hand, used when no dashboard is set.
    pub panels: Vec<Panel>,
    pub imported: Option<Imported>,
}

/// The file layout: every field but the URL and the data source is optional,
/// so a file written by hand stays short.
#[derive(Serialize, Deserialize)]
struct File {
    url: String,
    data_source: String,
    refresh_seconds: Option<u32>,
    dashboard: Option<String>,
    hidden: Option<Vec<String>>,
    panels: Option<Vec<Panel>>,
    imported: Option<Imported>,
}

impl Configuration {
    pub fn parse(text: &str) -> Result<Self> {
        let file: File = json::from_str(text).map_err(|_| Error::Configuration)?;
        Ok(Self {
            url: file.url,
            data_source: file.data_source,
            refresh_seconds: file.refresh_seconds.unwrap_or(DEFAULT_REFRESH_SECONDS),
            dashboard: file.dashboard.filter(|uid| !uid.trim().is_empty()),
            hidden: file.hidden.unwrap_or_default(),
            panels: file.panels.unwrap_or_default(),
            imported: file.imported,
        })
    }

    pub fn to_json(&self) -> String {
        json::to_string(&File {
            url: self.url.clone(),
            data_source: self.data_source.clone(),
            refresh_seconds: Some(self.refresh_seconds),
            dashboard: self.dashboard.clone(),
            hidden: Some(self.hidden.clone()),
            panels: Some(self.panels.clone()),
            imported: self.imported.clone(),
        })
    }

    pub fn refresh_seconds(&self) -> u32 {
        self.refresh_seconds.max(MINIMUM_REFRESH_SECONDS)
    }

    /// Checks the connection fields and every manual panel.
    pub fn validate(&self) -> core::result::Result<(), Invalid> {
        validate_url(&self.url)?;
        if self.data_source.trim().is_empty() {
            return Err(Invalid::DataSource);
        }
        if self.refresh_seconds < MINIMUM_REFRESH_SECONDS {
            return Err(Invalid::Refresh);
        }
        if let Some(uid) = &self.dashboard
            && !is_valid_uid(uid)
        {
            return Err(Invalid::Dashboard);
        }
        if self.panels.len() > MAXIMUM_PANELS {
            return Err(Invalid::TooManyPanels);
        }
        self.panels.iter().try_for_each(Panel::validate)
    }

    pub fn is_hidden(&self, panel: &Panel) -> bool {
        self.hidden.iter().any(|key| key == panel.hiding_key())
    }

    pub fn set_hidden(&mut self, key: &str, hidden: bool) {
        self.hidden.retain(|existing| existing != key);
        if hidden {
            self.hidden.push(String::from(key));
        }
    }
}

/// Accepts `http://` and `https://` URLs with a host and no spaces.
pub fn validate_url(url: &str) -> core::result::Result<(), Invalid> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or(Invalid::Url)?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty()
        || url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(Invalid::Url);
    }
    Ok(())
}

/// Grafana dashboard uids are short and URL-safe.
pub fn is_valid_uid(uid: &str) -> bool {
    !uid.is_empty()
        && uid.len() <= 64
        && uid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

/// Parses an optional number typed in a text field; empty means `None`.
pub fn parse_optional_number(text: &str) -> core::result::Result<Option<f64>, Invalid> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match text.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(Some(value)),
        _ => Err(Invalid::Threshold),
    }
}

/// Text for an optional number in a text field.
pub fn number_text(value: Option<f64>) -> String {
    match value {
        None => String::new(),
        Some(value) => {
            let mut text = crate::format::format_fixed(value, MAXIMUM_DECIMALS);
            if text.contains('.') {
                while text.ends_with('0') {
                    text.pop();
                }
                if text.ends_with('.') {
                    text.pop();
                }
            }
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Configuration {
        Configuration {
            url: String::from("https://grafana.example.com"),
            data_source: String::from("prometheus"),
            refresh_seconds: 30,
            dashboard: Some(String::from("hosts")),
            hidden: std::vec![String::from("7")],
            panels: std::vec![Panel {
                title: String::from("CPU \"busy\""),
                expression: String::from("avg(rate(node_cpu_seconds_total{mode!=\"idle\"}[5m]))"),
                unit: Some(Unit::Fraction),
                decimals: Some(1),
                warning: Some(0.8),
                critical: Some(0.9),
                kind: Some(Kind::Chart),
                range_seconds: Some(3_600),
                ..Panel::new()
            }],
            imported: Some(Imported {
                uid: String::from("hosts"),
                title: String::from("Hosts"),
                skipped: 2,
                panels: std::vec![Panel {
                    key: Some(String::from("7")),
                    title: String::from("Uptime"),
                    expression: String::from("time() - node_boot_time_seconds"),
                    unit: Some(Unit::Seconds),
                    ..Panel::new()
                }],
            }),
        }
    }

    #[test]
    fn round_trips_through_the_file_format() {
        let configuration = example();
        assert_eq!(
            Configuration::parse(&configuration.to_json()).unwrap(),
            configuration
        );
    }

    #[test]
    fn hand_written_files_only_need_the_connection() {
        let configuration = Configuration::parse(
            r#"{"url": "https://grafana.example.com", "data_source": "prometheus",
                "panels": [{"title": "Load", "expression": "node_load1", "unit": "none"}]}"#,
        )
        .unwrap();
        assert_eq!(configuration.refresh_seconds, DEFAULT_REFRESH_SECONDS);
        assert_eq!(configuration.dashboard, None);
        assert_eq!(configuration.panels[0].kind(), Kind::Value);
        assert_eq!(configuration.panels[0].unit(), Unit::None);
        assert_eq!(
            configuration.panels[0].range_seconds(),
            DEFAULT_RANGE_SECONDS
        );
        assert_eq!(configuration.validate(), Ok(()));
        assert_eq!(Configuration::parse("{}"), Err(Error::Configuration));
        assert_eq!(
            Configuration::parse(
                r#"{"url": "x", "data_source": "y", "panels": [{"title": "a", "expression": "b", "unit": "furlongs"}]}"#
            ),
            Err(Error::Configuration)
        );
    }

    #[test]
    fn validation_catches_user_mistakes() {
        let mut configuration = example();
        assert_eq!(configuration.validate(), Ok(()));
        configuration.url = String::from("ftp://grafana.example.com");
        assert_eq!(configuration.validate(), Err(Invalid::Url));
        configuration.url = String::from("https://");
        assert_eq!(configuration.validate(), Err(Invalid::Url));
        configuration.url = String::from("http://192.0.2.10:3000/grafana");
        assert_eq!(configuration.validate(), Ok(()));
        configuration.refresh_seconds = 2;
        assert_eq!(configuration.validate(), Err(Invalid::Refresh));
        configuration.refresh_seconds = 60;
        configuration.dashboard = Some(String::from("a/b"));
        assert_eq!(configuration.validate(), Err(Invalid::Dashboard));
        configuration.dashboard = None;
        configuration.panels[0].expression = String::from("  ");
        assert_eq!(configuration.validate(), Err(Invalid::Expression));
        configuration.panels[0].expression = String::from("up");
        configuration.panels[0].decimals = Some(9);
        assert_eq!(configuration.validate(), Err(Invalid::Decimals));
    }

    #[test]
    fn numbers_from_text_fields() {
        assert_eq!(parse_optional_number(" "), Ok(None));
        assert_eq!(parse_optional_number("0.8"), Ok(Some(0.8)));
        assert_eq!(parse_optional_number("-3e2"), Ok(Some(-300.0)));
        assert_eq!(parse_optional_number("8O"), Err(Invalid::Threshold));
        assert_eq!(parse_optional_number("inf"), Err(Invalid::Threshold));
        assert_eq!(number_text(Some(0.8)), "0.8");
        assert_eq!(number_text(Some(90.0)), "90");
        assert_eq!(number_text(None), "");
    }

    #[test]
    fn hidden_keys_use_the_id_then_the_title() {
        let mut configuration = example();
        let imported = configuration.imported.clone().unwrap();
        assert!(configuration.is_hidden(&imported.panels[0]));
        configuration.set_hidden("7", false);
        assert!(!configuration.is_hidden(&imported.panels[0]));
        let manual = &configuration.panels[0].clone();
        configuration.set_hidden(manual.hiding_key(), true);
        assert!(configuration.is_hidden(manual));
    }
}
