//! The IANA zone selector of the time zone section, compiled only with a
//! `time-zone-names-*` feature.
//!
//! The zones are grouped by region, the part of the name before the first `/`
//! (`Europe/Paris` is `Paris` in `Europe`); names without `/`, such as `UTC`
//! or `EST5EDT`, are in the last group. The selector keeps only the list of
//! regions (one `&str` per region, 11 groups with `iana-all`); the zone
//! dropdown is filled from the linked tables when the region changes, in table
//! order: sorted names, and with legacy aliases, the aliases of a region after
//! its canonical names. LVGL holds a copy of
//! the options of the selected region, at most 1,359 bytes for the 121
//! canonical names of `America`.
//! Applying a zone stores both its name and its TZ string; Core reads only the
//! TZ string.

use crate::error::Result;
use alloc::{string::String, vec::Vec};
use xila::graphics::{Event, EventKind, lvgl};
use xila::internationalization::t;
use xila::time_zone::{TimeZone, iana};

/// The region of a zone name, or "" for a name without `/`.
pub(crate) fn region_of(name: &str) -> &str {
    name.split_once('/').map_or("", |(region, _)| region)
}

/// The regions of the linked table, sorted, with "" (names without `/`) last.
pub(crate) fn regions() -> Vec<&'static str> {
    let mut regions: Vec<&'static str> = Vec::new();
    for index in 0..iana::zone_count() {
        let Some(name) = iana::zone_name(index) else {
            continue;
        };
        let region = region_of(name);
        if let Err(position) = regions.binary_search(&region) {
            regions.insert(position, region);
        }
    }
    if regions.first() == Some(&"") {
        regions.remove(0);
        regions.push("");
    }
    regions
}

/// The names of the zones in a region, in table order.
pub(crate) fn zones_in(region: &str) -> impl Iterator<Item = &'static str> + '_ {
    (0..iana::zone_count())
        .filter_map(iana::zone_name)
        .filter(move |name| region_of(name) == region)
}

/// The dropdown options of a region: the zone names without the region,
/// separated by `\n`, ending with `\0`.
pub(crate) fn options(region: &str) -> String {
    let mut options = String::new();
    for name in zones_in(region) {
        if !options.is_empty() {
            options.push('\n');
        }
        options.push_str(if region.is_empty() {
            name
        } else {
            &name[region.len() + 1..]
        });
    }
    options.push('\0');
    options
}

/// What the section does after a selector event.
pub enum Action {
    /// The event is not for the selector.
    None,
    /// The selector handled the event.
    Handled,
    /// Show this zone in the preview.
    Preview(TimeZone),
    /// Store this zone.
    Apply {
        name: &'static str,
        time_zone: &'static str,
    },
}

pub struct ZoneSelector {
    region: *mut lvgl::lv_obj_t,
    zone: *mut lvgl::lv_obj_t,
    apply: *mut lvgl::lv_obj_t,
    regions: Vec<&'static str>,
}

impl ZoneSelector {
    pub fn new() -> Self {
        Self {
            region: core::ptr::null_mut(),
            zone: core::ptr::null_mut(),
            apply: core::ptr::null_mut(),
            regions: Vec::new(),
        }
    }

    /// Build the selector in `parent` and select `current`, if the table has
    /// it.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    pub unsafe fn create_ui(
        &mut self,
        parent: *mut lvgl::lv_obj_t,
        current: Option<&str>,
    ) -> Result<()> {
        self.regions = regions();
        let mut region_options = String::new();
        for region in &self.regions {
            if !region_options.is_empty() {
                region_options.push('\n');
            }
            if region.is_empty() {
                region_options.push_str(t!("general.time_zone.other_region"));
            } else {
                region_options.push_str(region);
            }
        }
        region_options.push('\0');

        let selected_region = current
            .filter(|name| iana::contains(name))
            .and_then(|name| {
                let region = region_of(name);
                let index = self.regions.iter().position(|item| *item == region)?;
                let zone = zones_in(region).position(|item| item == name)?;
                Some((index, zone))
            });

        unsafe {
            super::caption(parent, t!(c"general.time_zone.region"))?;
            self.region = super::check(lvgl::lv_dropdown_create(parent))?;
            lvgl::lv_obj_set_width(self.region, lvgl::lv_pct(100));
            lvgl::lv_dropdown_set_options(self.region, region_options.as_ptr().cast());

            super::caption(parent, t!(c"general.time_zone.zone"))?;
            self.zone = super::check(lvgl::lv_dropdown_create(parent))?;
            lvgl::lv_obj_set_width(self.zone, lvgl::lv_pct(100));

            let (region, zone) = selected_region.unwrap_or((0, 0));
            lvgl::lv_dropdown_set_selected(self.region, region as u32);
            self.fill_zones();
            lvgl::lv_dropdown_set_selected(self.zone, zone as u32);

            self.apply = super::check(lvgl::lv_button_create(parent))?;
            let label = super::check(lvgl::lv_label_create(self.apply))?;
            lvgl::lv_label_set_text(label, t!(c"general.time_zone.apply_zone").as_ptr());
            lvgl::lv_obj_center(label);
        }
        Ok(())
    }

    fn selected_region(&self) -> &'static str {
        // SAFETY: the dropdown is live and the caller holds the graphics lock.
        let index = unsafe { lvgl::lv_dropdown_get_selected(self.region) } as usize;
        self.regions.get(index).copied().unwrap_or("")
    }

    fn selected_zone(&self) -> Option<&'static str> {
        // SAFETY: the dropdown is live and the caller holds the graphics lock.
        let index = unsafe { lvgl::lv_dropdown_get_selected(self.zone) } as usize;
        zones_in(self.selected_region()).nth(index)
    }

    fn fill_zones(&mut self) {
        let options = options(self.selected_region());
        // SAFETY: the dropdown is live and the caller holds the graphics lock.
        // LVGL copies the options.
        unsafe {
            lvgl::lv_dropdown_set_options(self.zone, options.as_ptr().cast());
            lvgl::lv_dropdown_set_selected(self.zone, 0);
        }
    }

    fn preview(&self) -> Action {
        match self.selected_zone().and_then(TimeZone::from_iana_name) {
            Some(zone) => Action::Preview(zone),
            None => Action::Handled,
        }
    }

    /// Handle an event. The caller holds the graphics lock.
    pub fn handle_event(&mut self, event: &Event) -> Action {
        if event.code == EventKind::ValueChanged && event.is_target(self.region) {
            self.fill_zones();
            return self.preview();
        }
        if event.code == EventKind::ValueChanged && event.is_target(self.zone) {
            return self.preview();
        }
        if event.code == EventKind::Clicked && event.is_target(self.apply) {
            return match self
                .selected_zone()
                .and_then(|name| Some((name, iana::time_zone_string(name)?)))
            {
                Some((name, time_zone)) => Action::Apply { name, time_zone },
                None => Action::Handled,
            };
        }
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "time-zone-names-all")]
    #[test]
    fn regions_and_options() {
        let regions = regions();
        assert!(regions.contains(&"Europe"));
        assert_eq!(regions.last(), Some(&""));
        let europe = options("Europe");
        assert!(europe.starts_with("Andorra\n"));
        assert!(europe.contains("\nParis\n"));
        assert!(europe.ends_with('\0'));
        assert!(zones_in("").all(|name| !name.contains('/')));
        assert_eq!(region_of("America/Argentina/Salta"), "America");
        let paris = zones_in("Europe").position(|name| name == "Europe/Paris");
        assert!(paris.is_some());
    }

    #[cfg(all(
        feature = "time-zone-names-all",
        not(feature = "time-zone-names-legacy-aliases")
    ))]
    #[test]
    fn all_regions() {
        assert_eq!(
            regions(),
            [
                "Africa",
                "America",
                "Antarctica",
                "Asia",
                "Atlantic",
                "Australia",
                "Etc",
                "Europe",
                "Indian",
                "Pacific",
                ""
            ]
        );
    }
}
