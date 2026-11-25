//! The IANA zone selector of the time zone section, compiled only with a
//! `time-zone-names-*` feature.
//!
//! The zones are grouped by region, the part of the name before the first `/`
//! (`Europe/Paris` is `Paris` in `Europe`); names without `/`, such as `GMT`
//! or `EST5EDT`, are in the last group. The selector keeps the regions in a
//! fixed array of [`MAXIMUM_REGIONS`] name indexes (2 bytes each; 12 groups
//! with `iana-all`, 18 with the legacy aliases) and fills the zone dropdown from the linked tables when
//! the region changes, in table order. LVGL holds a copy of the zone list of
//! the selected region, at most 1,627 bytes (the 148 names of `America`).
//! Applying a zone stores both its name and its TZ string; Core reads only the
//! TZ string.
//!
//! When the stored name is not in the linked tables (a custom TZ string, or a
//! name of another build), no region is selected.

use crate::error::Result;
use alloc::string::String;
use xila::graphics::{Event, EventKind, lvgl};
use xila::internationalization::t;
use xila::time_zone::{TimeZone, iana};

/// The largest number of region groups kept.
pub(crate) const MAXIMUM_REGIONS: usize = 24;

/// The region of a zone name, or "" for a name without `/`.
pub(crate) fn region_of(name: &str) -> &str {
    match name.find('/') {
        Some(end) => &name[..end],
        None => "",
    }
}

/// The index of the names without `/` in [`Regions`].
const OTHER: u16 = u16::MAX;

/// The regions of the linked tables, in table order, with "" (names without
/// `/`) last. Each region is kept as the index of its first zone name.
pub(crate) struct Regions {
    first_names: [u16; MAXIMUM_REGIONS],
    count: usize,
}

impl Regions {
    pub(crate) fn new() -> Self {
        let mut regions = Self {
            first_names: [OTHER; MAXIMUM_REGIONS],
            count: 0,
        };
        let mut other = false;
        for index in 0..iana::zone_count() {
            let Some(name) = iana::zone_name(index) else {
                continue;
            };
            let region = region_of(name);
            if region.is_empty() {
                other = true;
            } else if regions.position(region).is_none() && regions.count < MAXIMUM_REGIONS {
                regions.first_names[regions.count] = index as u16;
                regions.count += 1;
            }
        }
        if other && regions.count < MAXIMUM_REGIONS {
            regions.first_names[regions.count] = OTHER;
            regions.count += 1;
        }
        regions
    }

    pub(crate) fn get(&self, index: usize) -> Option<&'static str> {
        match *self.first_names[..self.count].get(index)? {
            OTHER => Some(""),
            name => iana::zone_name(usize::from(name)).map(region_of),
        }
    }

    pub(crate) fn position(&self, region: &str) -> Option<usize> {
        let mut index = 0;
        while index < self.count {
            if self.get(index) == Some(region) {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    /// The region dropdown options, separated by `\n` and ended by `\0`.
    fn options(&self) -> String {
        let mut options = String::new();
        for index in 0..self.count {
            if index != 0 {
                options.push('\n');
            }
            let region = self.get(index).unwrap_or("");
            options.push_str(if region.is_empty() {
                t!("general.time_zone.other_region")
            } else {
                region
            });
        }
        options.push('\0');
        options
    }
}

/// The name at `position` among the zones of `region`, in table order.
pub(crate) fn zone_at(region: &str, position: usize) -> Option<&'static str> {
    let mut seen = 0;
    for index in 0..iana::zone_count() {
        let name = iana::zone_name(index)?;
        if region_of(name) == region {
            if seen == position {
                return Some(name);
            }
            seen += 1;
        }
    }
    None
}

/// The position of `name` among the zones of its region.
fn zone_position(name: &str) -> Option<usize> {
    let region = region_of(name);
    let mut seen = 0;
    for index in 0..iana::zone_count() {
        let candidate = iana::zone_name(index)?;
        if region_of(candidate) == region {
            if candidate == name {
                return Some(seen);
            }
            seen += 1;
        }
    }
    None
}

/// The zone dropdown options of a region: the names without the region,
/// separated by `\n` and ended by `\0`.
pub(crate) fn options(region: &str) -> String {
    let mut options = String::new();
    for index in 0..iana::zone_count() {
        let Some(name) = iana::zone_name(index) else {
            continue;
        };
        if region_of(name) != region {
            continue;
        }
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
    regions: Regions,
    /// Whether a region is selected: false until the user picks one when the
    /// stored name is not in the tables.
    chosen: bool,
}

impl ZoneSelector {
    /// Build the selector in `parent` and select `current`, if the tables
    /// have it.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    pub unsafe fn new(parent: *mut lvgl::lv_obj_t, current: Option<&str>) -> Result<Self> {
        let regions = Regions::new();
        let options = regions.options();
        let selected = current.and_then(|name| {
            let region = regions.position(region_of(name))?;
            Some((region, zone_position(name)?))
        });
        let mut selector = Self {
            region: core::ptr::null_mut(),
            zone: core::ptr::null_mut(),
            apply: core::ptr::null_mut(),
            regions,
            chosen: selected.is_some(),
        };
        unsafe {
            super::caption(parent, t!(c"general.time_zone.region"))?;
            selector.region = super::check(lvgl::lv_dropdown_create(parent))?;
            lvgl::lv_obj_set_width(selector.region, lvgl::lv_pct(100));
            lvgl::lv_dropdown_set_options(selector.region, options.as_ptr().cast());
            super::caption(parent, t!(c"general.time_zone.zone"))?;
            selector.zone = super::check(lvgl::lv_dropdown_create(parent))?;
            lvgl::lv_obj_set_width(selector.zone, lvgl::lv_pct(100));
            match selected {
                Some((region, zone)) => {
                    lvgl::lv_dropdown_set_selected(selector.region, region as u32);
                    selector.fill_zones();
                    lvgl::lv_dropdown_set_selected(selector.zone, zone as u32);
                }
                None => {
                    // No selection: an empty text instead of the first region.
                    lvgl::lv_dropdown_set_text(selector.region, c"".as_ptr());
                    lvgl::lv_dropdown_set_options(selector.zone, c"".as_ptr());
                }
            }
            selector.apply = super::check(lvgl::lv_button_create(parent))?;
            let label = super::check(lvgl::lv_label_create(selector.apply))?;
            lvgl::lv_label_set_text(label, t!(c"general.time_zone.apply_zone").as_ptr());
            lvgl::lv_obj_center(label);
        }
        Ok(selector)
    }

    fn selected_region(&self) -> &'static str {
        // SAFETY: the dropdown is live and the caller holds the graphics lock.
        let index = unsafe { lvgl::lv_dropdown_get_selected(self.region) } as usize;
        self.regions.get(index).unwrap_or("")
    }

    fn selected_zone(&self) -> Option<&'static str> {
        if !self.chosen {
            return None;
        }
        // SAFETY: the dropdown is live and the caller holds the graphics lock.
        let index = unsafe { lvgl::lv_dropdown_get_selected(self.zone) } as usize;
        zone_at(self.selected_region(), index)
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
            if !self.chosen {
                self.chosen = true;
                // SAFETY: the dropdown is live and the caller holds the
                // graphics lock. A null text shows the selected option again.
                unsafe { lvgl::lv_dropdown_set_text(self.region, core::ptr::null()) };
            }
            self.fill_zones();
            return self.preview();
        }
        if event.code == EventKind::ValueChanged && event.is_target(self.zone) {
            return self.preview();
        }
        if event.code == EventKind::Clicked && event.is_target(self.apply) {
            let Some(name) = self.selected_zone() else {
                return Action::Handled;
            };
            return match iana::time_zone_string(name) {
                Some(time_zone) => Action::Apply { name, time_zone },
                None => Action::Handled,
            };
        }
        Action::None
    }
}

#[cfg(all(test, feature = "time-zone-names-all"))]
mod tests {
    use super::*;

    #[test]
    fn regions_and_options() {
        let regions = Regions::new();
        assert!(regions.position("Europe").is_some());
        assert_eq!(regions.get(regions.count - 1), Some(""));
        let europe = options("Europe");
        assert!(europe.starts_with("Amsterdam\n"));
        assert!(europe.contains("\nParis\n"));
        assert!(europe.ends_with('\0'));
        assert_eq!(region_of("America/Argentina/Salta"), "America");
        let paris = zone_position("Europe/Paris").unwrap();
        assert_eq!(zone_at("Europe", paris), Some("Europe/Paris"));
        assert_eq!(zone_position("Not/AZone"), None);
    }

    #[cfg(not(feature = "time-zone-names-legacy-aliases"))]
    #[test]
    fn all_regions() {
        let regions = Regions::new();
        let names: alloc::vec::Vec<_> = (0..regions.count).filter_map(|index| regions.get(index)).collect();
        assert_eq!(
            names,
            [
                "Africa",
                "America",
                "Antarctica",
                "Arctic",
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
