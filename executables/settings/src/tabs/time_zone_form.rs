//! The offset form of the time zone section: a lightweight manual UTC offset
//! form that builds a POSIX TZ string, so that nobody has to type the string.
//! It needs no zone table.
//!
//! - The standard offset is in hours and minutes east of UTC (UTC-12:00 to
//!   UTC+14:00, the range of the zones in use, in steps of 15 minutes). POSIX
//!   counts offsets west of UTC, so UTC+01:00 is written `-1`, and the
//!   abbreviation is always the clean numeric tzdata form (`<+01>`), for
//!   example `<+01>-1`.
//! - Daylight saving time is off or on (one switch). When on, the daylight
//!   offset is one hour ahead of standard time and the rules come from a
//!   preset, built only when the switch is first turned on:
//!   - "European Union": last Sunday of March to last Sunday of October, at
//!     01:00 UTC, written as the local times of the chosen offset (02:00 and
//!     03:00 for UTC+01:00).
//!   - "United States": second Sunday of March to first Sunday of November,
//!     at 02:00 local time.
//!
//!   For UTC+01:00 with the European Union rules, the generated string is
//!   `<+01>-1<+02>,M3.5.0,M10.5.0/3`; for UTC-05:00 with the United States
//!   rules, `<-05>5<-04>,M3.2.0,M11.1.0`.
//! - The form shows the generated string live and stores it with the "Use
//!   this offset" button.
//!
//! Custom rules, unusual shifts and abbreviations remain available in the
//! adjacent "TZ string (advanced)" editor.

use crate::error::Result;
use alloc::string::String;
use core::fmt::Write;
use xila::graphics::{Event, EventKind, lvgl};
use xila::internationalization::t;
use xila::time_zone::TimeZone;

/// The smallest and largest offsets that the form offers, in hours.
const MINIMUM_OFFSET_HOURS: i16 = -12;
const MAXIMUM_OFFSET_HOURS: i16 = 14;
/// The minutes that the offset can have.
const OFFSET_MINUTES: [i16; 4] = [0, 15, 30, 45];
/// The daylight saving shift of both presets: one hour ahead of standard time.
const DST_SHIFT_MINUTES: i16 = 60;

/// A transition of a daylight saving rule: the `Mm.w.d/time` form of POSIX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transition {
    /// 1 to 12.
    pub month: u8,
    /// 1 to 4, or 5 for the last week.
    pub week: u8,
    /// 0 (Sunday) to 6.
    pub weekday: u8,
    /// Local time of the transition, in minutes after midnight.
    pub time_minutes: i16,
}

/// The daylight saving rule presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rules {
    EuropeanUnion,
    UnitedStates,
}

/// The values of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Values {
    /// The standard offset east of UTC, in minutes.
    pub offset_minutes: i16,
    /// The daylight saving rules, when daylight saving time is on.
    pub daylight_saving: Option<Rules>,
}

/// Why the form cannot give a TZ string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormError {
    /// The offset is outside UTC-12:00 to UTC+14:00.
    OffsetOutOfRange,
    /// The string does not parse.
    Invalid,
}

impl Rules {
    /// The start and end transitions for a standard offset.
    pub(crate) fn transitions(&self, offset_minutes: i16) -> (Transition, Transition) {
        let sunday = |month, week, time_minutes| Transition {
            month,
            week,
            weekday: 0,
            time_minutes,
        };
        match *self {
            // 01:00 UTC is 01:00 plus the standard offset before the change,
            // and 01:00 plus the daylight offset before the change back.
            Rules::EuropeanUnion => (
                sunday(3, 5, 60 + offset_minutes),
                sunday(10, 5, 60 + offset_minutes + DST_SHIFT_MINUTES),
            ),
            Rules::UnitedStates => (sunday(3, 2, 120), sunday(11, 1, 120)),
        }
    }
}

/// Write a duration in minutes as POSIX `[-]h[:mm]`.
fn write_hours(output: &mut String, minutes: i16) {
    if minutes < 0 {
        output.push('-');
    }
    let magnitude = minutes.unsigned_abs();
    let _ = write!(output, "{}", magnitude / 60);
    if !magnitude.is_multiple_of(60) {
        let _ = write!(output, ":{:02}", magnitude % 60);
    }
}

/// Write the numeric tzdata form of an offset: `<+01>`, `<+0530>`, `<-03>`.
fn write_numeric_name(output: &mut String, offset_minutes: i16) {
    let magnitude = offset_minutes.unsigned_abs();
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let _ = write!(output, "<{sign}{:02}", magnitude / 60);
    if !magnitude.is_multiple_of(60) {
        let _ = write!(output, "{:02}", magnitude % 60);
    }
    output.push('>');
}

fn write_transition(output: &mut String, transition: &Transition) {
    let _ = write!(
        output,
        ",M{}.{}.{}",
        transition.month, transition.week, transition.weekday
    );
    // POSIX defaults to 02:00.
    if transition.time_minutes != 120 {
        output.push('/');
        write_hours(output, transition.time_minutes);
    }
}

/// Build and check the TZ string of the form.
pub(crate) fn build(values: &Values) -> core::result::Result<String, FormError> {
    if !(MINIMUM_OFFSET_HOURS * 60..=MAXIMUM_OFFSET_HOURS * 60).contains(&values.offset_minutes) {
        return Err(FormError::OffsetOutOfRange);
    }
    let mut output = String::new();
    write_numeric_name(&mut output, values.offset_minutes);
    write_hours(&mut output, -values.offset_minutes);
    if let Some(rules) = values.daylight_saving {
        write_numeric_name(&mut output, values.offset_minutes + DST_SHIFT_MINUTES);
        // POSIX defaults to one hour ahead of standard time: no value.
        let (start, end) = rules.transitions(values.offset_minutes);
        write_transition(&mut output, &start);
        write_transition(&mut output, &end);
    }
    xila::local_time::validate_time_zone(&output).map_err(|_| FormError::Invalid)?;
    Ok(output)
}

/// Dropdown options separated by `\n` and ended by `\0`.
fn options(count: usize, mut item: impl FnMut(&mut String, usize)) -> String {
    let mut options = String::new();
    for index in 0..count {
        if index != 0 {
            options.push('\n');
        }
        item(&mut options, index);
    }
    options.push('\0');
    options
}

/// A dropdown under a caption, as wide as its parent.
///
/// # Safety
/// `parent` must be live and the caller must hold the graphics lock.
unsafe fn dropdown(
    parent: *mut lvgl::lv_obj_t,
    caption: &core::ffi::CStr,
    options: &str,
) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        super::caption(parent, caption)?;
        let dropdown = super::check(lvgl::lv_dropdown_create(parent))?;
        lvgl::lv_obj_set_width(dropdown, lvgl::lv_pct(100));
        lvgl::lv_dropdown_set_options(dropdown, options.as_ptr().cast());
        Ok(dropdown)
    }
}

/// A column that holds a part of the form.
///
/// # Safety
/// `parent` must be live and the caller must hold the graphics lock.
unsafe fn column(parent: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let column = super::check(lvgl::lv_obj_create(parent))?;
        lvgl::lv_obj_remove_style_all(column);
        lvgl::lv_obj_set_size(column, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
        lvgl::lv_obj_set_flex_flow(column, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
        lvgl::lv_obj_set_style_pad_row(column, 6, 0);
        Ok(column)
    }
}

/// Show or hide an object.
///
/// # Safety
/// `object` must be live and the caller must hold the graphics lock.
unsafe fn set_visible(object: *mut lvgl::lv_obj_t, visible: bool) {
    unsafe {
        if visible {
            lvgl::lv_obj_remove_flag(object, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        } else {
            lvgl::lv_obj_add_flag(object, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        }
    }
}

/// What the section does after a form event.
pub enum Action {
    None,
    /// The string changed: show it in the preview if it is valid.
    Changed(core::result::Result<TimeZone, FormError>),
    /// Store this string.
    Apply(core::result::Result<String, FormError>),
}

/// The offset form: two offset dropdowns, one daylight saving switch with a
/// lazily built preset dropdown, the generated string and an apply button.
///
/// On a 32-bit target it holds 7 widget pointers (28 bytes).
pub struct OffsetForm {
    hours: *mut lvgl::lv_obj_t,
    minutes: *mut lvgl::lv_obj_t,
    daylight_saving: *mut lvgl::lv_obj_t,
    /// Empty until daylight saving time is first turned on.
    daylight_container: *mut lvgl::lv_obj_t,
    /// Null until daylight saving time is first turned on.
    preset: *mut lvgl::lv_obj_t,
    generated: *mut lvgl::lv_obj_t,
    apply: *mut lvgl::lv_obj_t,
}

impl OffsetForm {
    /// Build the form in `parent`, set to UTC, without daylight saving time.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    pub unsafe fn new(parent: *mut lvgl::lv_obj_t) -> Result<Self> {
        let hours = options(
            (MAXIMUM_OFFSET_HOURS - MINIMUM_OFFSET_HOURS + 1) as usize,
            |output, index| {
                let hour = index as i16 + MINIMUM_OFFSET_HOURS;
                let sign = if hour < 0 { '-' } else { '+' };
                let _ = write!(output, "UTC{sign}{:02}", hour.unsigned_abs());
            },
        );
        let mut form = Self {
            hours: core::ptr::null_mut(),
            minutes: core::ptr::null_mut(),
            daylight_saving: core::ptr::null_mut(),
            daylight_container: core::ptr::null_mut(),
            preset: core::ptr::null_mut(),
            generated: core::ptr::null_mut(),
            apply: core::ptr::null_mut(),
        };
        unsafe {
            form.hours = dropdown(parent, t!(c"general.time_zone.form.offset"), &hours)?;
            lvgl::lv_dropdown_set_selected(form.hours, (-MINIMUM_OFFSET_HOURS) as u32);
            form.minutes = dropdown(
                parent,
                t!(c"general.time_zone.form.minutes"),
                ":00\n:15\n:30\n:45\0",
            )?;
            let row = super::setting_row(parent, t!(c"general.time_zone.form.daylight_saving"))?;
            form.daylight_saving = super::check(lvgl::lv_switch_create(row))?;
            form.daylight_container = column(parent)?;
            set_visible(form.daylight_container, false);
            super::caption(parent, t!(c"general.time_zone.form.generated"))?;
            form.generated = super::check(lvgl::lv_label_create(parent))?;
            lvgl::lv_obj_set_width(form.generated, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                form.generated,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );
            form.apply = super::check(lvgl::lv_button_create(parent))?;
            let label = super::check(lvgl::lv_label_create(form.apply))?;
            lvgl::lv_label_set_text(label, t!(c"general.time_zone.form.apply").as_ptr());
            lvgl::lv_obj_center(label);
        }
        let _ = form.update_generated();
        Ok(form)
    }

    /// Build the preset dropdown, once.
    ///
    /// # Safety
    /// The caller must hold the graphics lock.
    unsafe fn build_preset(&mut self) -> Result<()> {
        if !self.preset.is_null() {
            return Ok(());
        }
        let presets = options(2, |output, index| {
            output.push_str(match index {
                0 => t!("general.time_zone.form.presets.european_union"),
                _ => t!("general.time_zone.form.presets.united_states"),
            })
        });
        unsafe {
            self.preset = dropdown(
                self.daylight_container,
                t!(c"general.time_zone.form.rules"),
                &presets,
            )?;
        }
        Ok(())
    }

    fn daylight_saving_enabled(&self) -> bool {
        // SAFETY: the switch is live and the caller holds the graphics lock.
        unsafe { lvgl::lv_obj_has_state(self.daylight_saving, lvgl::LV_STATE_CHECKED) }
    }

    /// The TZ string of the current values.
    ///
    /// The caller must hold the graphics lock.
    pub fn build(&self) -> core::result::Result<String, FormError> {
        // SAFETY: the widgets are live (or null until built) and the caller
        // holds the graphics lock.
        unsafe {
            let hours = lvgl::lv_dropdown_get_selected(self.hours) as i16 + MINIMUM_OFFSET_HOURS;
            let minutes = OFFSET_MINUTES[lvgl::lv_dropdown_get_selected(self.minutes) as usize % 4];
            let offset_minutes = if hours < 0 {
                hours * 60 - minutes
            } else {
                hours * 60 + minutes
            };
            let daylight_saving =
                (self.daylight_saving_enabled() && !self.preset.is_null()).then(|| {
                    match lvgl::lv_dropdown_get_selected(self.preset) {
                        1 => Rules::UnitedStates,
                        _ => Rules::EuropeanUnion,
                    }
                });
            build(&Values {
                offset_minutes,
                daylight_saving,
            })
        }
    }

    /// Build the preset dropdown if daylight saving time is on, and show
    /// only the daylight saving fields when it is.
    fn update_fields(&mut self) {
        let daylight = self.daylight_saving_enabled();
        // SAFETY: the widgets are live and the caller holds the graphics lock.
        unsafe {
            if daylight {
                let _ = self.build_preset();
            }
            set_visible(self.daylight_container, daylight);
        }
    }

    /// Show the generated string, or why there is none.
    fn update_generated(&mut self) -> core::result::Result<String, FormError> {
        let result = self.build();
        // SAFETY: the label is live and the caller holds the graphics lock.
        unsafe {
            match &result {
                Ok(string) => {
                    let mut text = string.clone();
                    text.push('\0');
                    lvgl::lv_label_set_text(self.generated, text.as_ptr().cast());
                }
                Err(_) => lvgl::lv_label_set_text(
                    self.generated,
                    t!(c"general.time_zone.errors.invalid").as_ptr(),
                ),
            }
            lvgl::lv_theme_shadcn_set_error(self.generated, result.is_err());
        }
        result
    }

    /// Set the offset in whole hours and the daylight saving switch, for tests.
    ///
    /// # Safety
    /// The widgets must be live and the caller must hold the graphics lock.
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) unsafe fn select(&mut self, offset_hours: i16, daylight_saving: bool) {
        unsafe {
            lvgl::lv_dropdown_set_selected(
                self.hours,
                (offset_hours - MINIMUM_OFFSET_HOURS) as u32,
            );
            if daylight_saving {
                lvgl::lv_obj_add_state(self.daylight_saving, lvgl::LV_STATE_CHECKED);
            } else {
                lvgl::lv_obj_remove_state(self.daylight_saving, lvgl::LV_STATE_CHECKED);
            }
        }
        self.update_fields();
    }

    /// Handle an event of the form. The caller holds the graphics lock.
    pub fn handle_event(&mut self, event: &Event) -> Action {
        if event.code == EventKind::Clicked && event.is_target(self.apply) {
            return Action::Apply(self.build());
        }
        if event.code != EventKind::ValueChanged {
            return Action::None;
        }
        let fields = [self.hours, self.minutes, self.daylight_saving, self.preset];
        if !fields
            .iter()
            .any(|field| !field.is_null() && event.is_target(*field))
        {
            return Action::None;
        }
        self.update_fields();
        Action::Changed(
            self.update_generated()
                .map(|string| TimeZone::parse(&string).unwrap_or_else(|_| xila::local_time::utc())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(offset_minutes: i16, daylight_saving: Option<Rules>) -> Values {
        Values {
            offset_minutes,
            daylight_saving,
        }
    }

    #[test]
    fn fixed_offsets_use_the_numeric_tzdata_form() {
        assert_eq!(build(&values(60, None)).unwrap(), "<+01>-1");
        assert_eq!(build(&values(330, None)).unwrap(), "<+0530>-5:30");
        assert_eq!(build(&values(345, None)).unwrap(), "<+0545>-5:45");
        assert_eq!(build(&values(-180, None)).unwrap(), "<-03>3");
        assert_eq!(build(&values(-210, None)).unwrap(), "<-0330>3:30");
        assert_eq!(build(&values(0, None)).unwrap(), "<+00>0");
        assert_eq!(build(&values(-12 * 60, None)).unwrap(), "<-12>12");
        assert_eq!(build(&values(14 * 60, None)).unwrap(), "<+14>-14");
    }

    #[test]
    fn offsets_outside_the_zones_in_use_are_refused() {
        assert_eq!(
            build(&values(14 * 60 + 15, None)),
            Err(FormError::OffsetOutOfRange)
        );
        assert_eq!(
            build(&values(-12 * 60 - 30, None)),
            Err(FormError::OffsetOutOfRange)
        );
    }

    #[test]
    fn the_european_union_preset_matches_tzdata() {
        let european_union = Some(Rules::EuropeanUnion);
        // Europe/Paris: CET-1CEST,M3.5.0,M10.5.0/3.
        assert_eq!(
            build(&values(60, european_union)).unwrap(),
            "<+01>-1<+02>,M3.5.0,M10.5.0/3"
        );
        // Europe/London: GMT0BST,M3.5.0/1,M10.5.0.
        assert_eq!(
            build(&values(0, european_union)).unwrap(),
            "<+00>0<+01>,M3.5.0/1,M10.5.0"
        );
        // Europe/Athens: EET-2EEST,M3.5.0/3,M10.5.0/4.
        assert_eq!(
            build(&values(120, european_union)).unwrap(),
            "<+02>-2<+03>,M3.5.0/3,M10.5.0/4"
        );
        // Atlantic/Azores: <-01>1<+00>,M3.5.0/0,M10.5.0/1.
        assert_eq!(
            build(&values(-60, european_union)).unwrap(),
            "<-01>1<+00>,M3.5.0/0,M10.5.0/1"
        );
    }

    #[test]
    fn the_united_states_preset_matches_tzdata() {
        let united_states = Some(Rules::UnitedStates);
        // America/New_York: EST5EDT,M3.2.0,M11.1.0.
        assert_eq!(
            build(&values(-300, united_states)).unwrap(),
            "<-05>5<-04>,M3.2.0,M11.1.0"
        );
        // America/St_Johns: NST3:30NDT,M3.2.0,M11.1.0.
        assert_eq!(
            build(&values(-210, united_states)).unwrap(),
            "<-0330>3:30<-0230>,M3.2.0,M11.1.0"
        );
    }

    #[test]
    fn the_built_zones_convert_like_tzdata_at_the_2026_changes() {
        // European Union: 2026-03-29 01:00 UTC and 2026-10-25 01:00 UTC.
        let built =
            TimeZone::parse(&build(&values(60, Some(Rules::EuropeanUnion))).unwrap()).unwrap();
        let paris = TimeZone::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        for utc in [1_774_745_999, 1_774_746_000, 1_792_889_999, 1_792_890_000] {
            assert_eq!(built.offset_at(utc), paris.offset_at(utc), "{utc}");
        }
        assert_eq!(built.from_utc(1_774_746_000).abbreviation, "+02");
        // United States: 2026-03-08 07:00 UTC and 2026-11-01 06:00 UTC.
        let built =
            TimeZone::parse(&build(&values(-300, Some(Rules::UnitedStates))).unwrap()).unwrap();
        let new_york = TimeZone::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
        for utc in [1_772_953_199, 1_772_953_200, 1_793_512_799, 1_793_512_800] {
            assert_eq!(built.offset_at(utc), new_york.offset_at(utc), "{utc}");
        }
        assert_eq!(built.from_utc(1_772_953_200).abbreviation, "-04");
    }
}
