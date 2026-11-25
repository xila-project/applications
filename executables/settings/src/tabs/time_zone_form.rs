//! The offset form of the time zone section: it builds a POSIX TZ string from
//! a UTC offset, optional daylight saving rules and optional abbreviations, so
//! that nobody has to type the string. It needs no zone table.
//!
//! - The offset is in hours and minutes east of UTC (UTC+01:00, UTC-03:30).
//!   POSIX counts offsets west of UTC, so UTC+01:00 is written `-1`.
//! - Daylight saving time is off or on. When on, the daylight offset is 30
//!   minutes, one hour (the default) or two hours ahead of standard time, and
//!   the rules come from a preset or from the custom fields:
//!   - "European Union": last Sunday of March to last Sunday of October, at
//!     01:00 UTC, written as the local times of the chosen offset (02:00 and
//!     03:00 for UTC+01:00).
//!   - "United States": second Sunday of March to first Sunday of November,
//!     at 02:00 local time.
//!   - Custom: month, week (first to fourth, or last), weekday and hour of the
//!     start and of the end, in local time (standard time for the start,
//!     daylight time for the end, as POSIX defines).
//! - Abbreviations are optional (`CET`, `CEST`). When empty, the numeric form
//!   of tzdata is used (`<+01>`, `<+0530>`).

use crate::error::Result;
use alloc::string::String;
use core::fmt::Write;
use xila::graphics::{Event, EventKind, lvgl};
use xila::internationalization::{self, t};
use xila::time_zone::TimeZone;

/// The smallest and largest offsets that the form offers, in hours.
const MINIMUM_OFFSET_HOURS: i16 = -12;
const MAXIMUM_OFFSET_HOURS: i16 = 14;
/// The minutes that the offset can have.
const OFFSET_MINUTES: [i16; 4] = [0, 15, 30, 45];
/// The daylight saving shifts that the form offers, in minutes.
const SHIFTS: [i16; 3] = [30, 60, 120];

/// A transition of a daylight saving rule: the `Mm.w.d/time` form of POSIX.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transition {
    /// 1 to 12.
    pub month: u8,
    /// 1 to 4, or 5 for the last week.
    pub week: u8,
    /// 0 (Sunday) to 6.
    pub weekday: u8,
    /// Local time of the transition, in minutes after midnight. It can be
    /// negative or above 24 hours, as POSIX allows.
    pub time_minutes: i16,
}

/// Where the daylight saving rules come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rules {
    EuropeanUnion,
    UnitedStates,
    Custom { start: Transition, end: Transition },
}

/// The daylight saving part of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DaylightSaving {
    /// How far daylight time is ahead of standard time, in minutes.
    pub shift_minutes: i16,
    pub rules: Rules,
}

/// The values of the form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Values<'a> {
    /// The standard offset east of UTC, in minutes.
    pub offset_minutes: i16,
    pub daylight_saving: Option<DaylightSaving>,
    /// The standard abbreviation, or "" for the numeric form.
    pub standard_name: &'a str,
    /// The daylight abbreviation, or "" for the numeric form.
    pub daylight_name: &'a str,
}

/// Why the form cannot give a TZ string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FormError {
    /// An abbreviation is not 3 to 7 ASCII letters, digits, `+` or `-`.
    InvalidAbbreviation,
    /// The string does not parse (for example a rule time out of range).
    Invalid,
}

impl Rules {
    /// The start and end transitions for a standard offset and a shift.
    pub(crate) fn transitions(
        &self,
        offset_minutes: i16,
        shift_minutes: i16,
    ) -> (Transition, Transition) {
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
                sunday(10, 5, 60 + offset_minutes + shift_minutes),
            ),
            Rules::UnitedStates => (sunday(3, 2, 120), sunday(11, 1, 120)),
            Rules::Custom { start, end } => (start, end),
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

/// Write an abbreviation: as given when it is letters only, quoted when it has
/// digits or signs, or the numeric tzdata form of `offset_minutes` when empty.
fn write_name(
    output: &mut String,
    name: &str,
    offset_minutes: i16,
) -> core::result::Result<(), FormError> {
    if name.is_empty() {
        let magnitude = offset_minutes.unsigned_abs();
        let sign = if offset_minutes < 0 { '-' } else { '+' };
        let _ = write!(output, "<{sign}{:02}", magnitude / 60);
        if !magnitude.is_multiple_of(60) {
            let _ = write!(output, "{:02}", magnitude % 60);
        }
        output.push('>');
        return Ok(());
    }
    if !(3..=7).contains(&name.len())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'-')
    {
        return Err(FormError::InvalidAbbreviation);
    }
    if name.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        output.push_str(name);
    } else {
        let _ = write!(output, "<{name}>");
    }
    Ok(())
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
pub(crate) fn build(values: &Values<'_>) -> core::result::Result<String, FormError> {
    let mut output = String::new();
    write_name(&mut output, values.standard_name, values.offset_minutes)?;
    write_hours(&mut output, -values.offset_minutes);
    if let Some(daylight_saving) = values.daylight_saving {
        let daylight_offset = values.offset_minutes + daylight_saving.shift_minutes;
        write_name(&mut output, values.daylight_name, daylight_offset)?;
        // POSIX defaults to one hour ahead of standard time.
        if daylight_saving.shift_minutes != 60 {
            write_hours(&mut output, -daylight_offset);
        }
        let (start, end) = daylight_saving
            .rules
            .transitions(values.offset_minutes, daylight_saving.shift_minutes);
        write_transition(&mut output, &start);
        write_transition(&mut output, &end);
    }
    xila::local_time::validate_time_zone(&output).map_err(|_| FormError::Invalid)?;
    Ok(output)
}

/// Dropdown options separated by `\n` and ended by `\0`.
fn options(items: impl Iterator<Item = String>) -> String {
    let mut options = String::new();
    for item in items {
        if !options.is_empty() {
            options.push('\n');
        }
        options.push_str(&item);
    }
    options.push('\0');
    options
}

/// The widgets of one custom transition.
#[derive(Clone, Copy)]
struct TransitionFields {
    month: *mut lvgl::lv_obj_t,
    week: *mut lvgl::lv_obj_t,
    weekday: *mut lvgl::lv_obj_t,
    hour: *mut lvgl::lv_obj_t,
}

impl TransitionFields {
    const fn null() -> Self {
        Self {
            month: core::ptr::null_mut(),
            week: core::ptr::null_mut(),
            weekday: core::ptr::null_mut(),
            hour: core::ptr::null_mut(),
        }
    }

    fn contains(&self, object: &Event) -> bool {
        [self.month, self.week, self.weekday, self.hour]
            .iter()
            .any(|field| object.is_target(*field))
    }

    /// # Safety
    /// The fields must be live and the caller must hold the graphics lock.
    unsafe fn read(&self) -> Transition {
        unsafe {
            Transition {
                month: lvgl::lv_dropdown_get_selected(self.month) as u8 + 1,
                week: lvgl::lv_dropdown_get_selected(self.week) as u8 + 1,
                weekday: lvgl::lv_dropdown_get_selected(self.weekday) as u8,
                time_minutes: lvgl::lv_dropdown_get_selected(self.hour) as i16 * 60,
            }
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

pub struct OffsetForm {
    container: *mut lvgl::lv_obj_t,
    hours: *mut lvgl::lv_obj_t,
    minutes: *mut lvgl::lv_obj_t,
    daylight_saving: *mut lvgl::lv_obj_t,
    daylight_container: *mut lvgl::lv_obj_t,
    shift: *mut lvgl::lv_obj_t,
    preset: *mut lvgl::lv_obj_t,
    custom_container: *mut lvgl::lv_obj_t,
    start: TransitionFields,
    end: TransitionFields,
    standard_name: *mut lvgl::lv_obj_t,
    daylight_name: *mut lvgl::lv_obj_t,
    generated: *mut lvgl::lv_obj_t,
    apply: *mut lvgl::lv_obj_t,
}

impl OffsetForm {
    pub const fn new() -> Self {
        Self {
            container: core::ptr::null_mut(),
            hours: core::ptr::null_mut(),
            minutes: core::ptr::null_mut(),
            daylight_saving: core::ptr::null_mut(),
            daylight_container: core::ptr::null_mut(),
            shift: core::ptr::null_mut(),
            preset: core::ptr::null_mut(),
            custom_container: core::ptr::null_mut(),
            start: TransitionFields::null(),
            end: TransitionFields::null(),
            standard_name: core::ptr::null_mut(),
            daylight_name: core::ptr::null_mut(),
            generated: core::ptr::null_mut(),
            apply: core::ptr::null_mut(),
        }
    }

    pub fn container(&self) -> *mut lvgl::lv_obj_t {
        self.container
    }

    /// Build the form in `parent`, set to UTC.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    pub unsafe fn create_ui(&mut self, parent: *mut lvgl::lv_obj_t) -> Result<()> {
        let formatter = internationalization::DateTimeFormatter::new(
            internationalization::date_time_locale(internationalization::locale!()),
        );
        // Month and weekday names come from the locale of the build:
        // 2024-01-14 (day 19,736) is a Sunday.
        // Days since 1970-01-01 of the first day of each month of 2024.
        const MONTH_STARTS: [i64; 12] = [
            19_723, 19_754, 19_783, 19_814, 19_844, 19_875, 19_905, 19_936, 19_967, 19_997, 20_028,
            20_058,
        ];
        let month_options = options(MONTH_STARTS.iter().map(|days| {
            let mut name = String::new();
            let _ = formatter.write(&mut name, (days + 14) * 86_400, "%B");
            name
        }));
        let weekday_options = options((0..7_i64).map(|weekday| {
            let mut name = String::new();
            let _ = formatter.write(&mut name, (19_736 + weekday) * 86_400, "%A");
            name
        }));
        let week_options = options(
            [
                t!("general.time_zone.form.weeks.first"),
                t!("general.time_zone.form.weeks.second"),
                t!("general.time_zone.form.weeks.third"),
                t!("general.time_zone.form.weeks.fourth"),
                t!("general.time_zone.form.weeks.last"),
            ]
            .into_iter()
            .map(String::from),
        );
        let hour_options = options((0..24).map(|hour| alloc::format!("{hour:02}:00")));

        unsafe {
            let dropdown =
                |parent, caption: &core::ffi::CStr, options: &str| -> Result<*mut lvgl::lv_obj_t> {
                    super::caption(parent, caption)?;
                    let dropdown = super::check(lvgl::lv_dropdown_create(parent))?;
                    lvgl::lv_obj_set_width(dropdown, lvgl::lv_pct(100));
                    lvgl::lv_dropdown_set_options(dropdown, options.as_ptr().cast());
                    Ok(dropdown)
                };
            let column = |parent| -> Result<*mut lvgl::lv_obj_t> {
                let column = super::check(lvgl::lv_obj_create(parent))?;
                lvgl::lv_obj_remove_style_all(column);
                lvgl::lv_obj_set_size(column, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
                lvgl::lv_obj_set_flex_flow(column, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
                lvgl::lv_obj_set_style_pad_row(column, 6, 0);
                Ok(column)
            };
            let text_area = |parent, caption: &core::ffi::CStr| -> Result<*mut lvgl::lv_obj_t> {
                super::caption(parent, caption)?;
                let text_area = super::check(lvgl::lv_textarea_create(parent))?;
                lvgl::lv_textarea_set_one_line(text_area, true);
                lvgl::lv_textarea_set_max_length(text_area, 7);
                lvgl::lv_obj_set_width(text_area, lvgl::lv_pct(100));
                Ok(text_area)
            };

            self.container = column(parent)?;
            let hours = options((MINIMUM_OFFSET_HOURS..=MAXIMUM_OFFSET_HOURS).map(|hour| {
                if hour < 0 {
                    alloc::format!("UTC-{:02}", -hour)
                } else {
                    alloc::format!("UTC+{hour:02}")
                }
            }));
            self.hours = dropdown(self.container, t!(c"general.time_zone.form.offset"), &hours)?;
            lvgl::lv_dropdown_set_selected(self.hours, (-MINIMUM_OFFSET_HOURS) as u32);
            let minutes = options(
                OFFSET_MINUTES
                    .iter()
                    .map(|minute| alloc::format!(":{minute:02}")),
            );
            self.minutes = dropdown(
                self.container,
                t!(c"general.time_zone.form.minutes"),
                &minutes,
            )?;

            let row = super::setting_row(
                self.container,
                t!(c"general.time_zone.form.daylight_saving"),
            )?;
            self.daylight_saving = super::check(lvgl::lv_switch_create(row))?;

            self.daylight_container = column(self.container)?;
            self.shift = dropdown(
                self.daylight_container,
                t!(c"general.time_zone.form.shift"),
                "+00:30\n+01:00\n+02:00\0",
            )?;
            lvgl::lv_dropdown_set_selected(self.shift, 1);
            let presets = options(
                [
                    t!("general.time_zone.form.presets.european_union"),
                    t!("general.time_zone.form.presets.united_states"),
                    t!("general.time_zone.form.presets.custom"),
                ]
                .into_iter()
                .map(String::from),
            );
            self.preset = dropdown(
                self.daylight_container,
                t!(c"general.time_zone.form.rules"),
                &presets,
            )?;

            self.custom_container = column(self.daylight_container)?;
            for (fields, caption, month, week) in [
                (&mut self.start, t!(c"general.time_zone.form.start"), 2, 4),
                (&mut self.end, t!(c"general.time_zone.form.end"), 9, 4),
            ] {
                let title = super::check(lvgl::lv_label_create(self.custom_container))?;
                lvgl::lv_label_set_text(title, caption.as_ptr());
                fields.month = dropdown(
                    self.custom_container,
                    t!(c"general.time_zone.form.month"),
                    &month_options,
                )?;
                lvgl::lv_dropdown_set_selected(fields.month, month);
                fields.week = dropdown(
                    self.custom_container,
                    t!(c"general.time_zone.form.week"),
                    &week_options,
                )?;
                lvgl::lv_dropdown_set_selected(fields.week, week);
                fields.weekday = dropdown(
                    self.custom_container,
                    t!(c"general.time_zone.form.weekday"),
                    &weekday_options,
                )?;
                fields.hour = dropdown(
                    self.custom_container,
                    t!(c"general.time_zone.form.time"),
                    &hour_options,
                )?;
                lvgl::lv_dropdown_set_selected(fields.hour, 2);
            }

            self.standard_name =
                text_area(self.container, t!(c"general.time_zone.form.standard_name"))?;
            self.daylight_name = text_area(
                self.daylight_container,
                t!(c"general.time_zone.form.daylight_name"),
            )?;
            super::caption(self.container, t!(c"general.time_zone.form.names_hint"))?;

            super::caption(self.container, t!(c"general.time_zone.form.generated"))?;
            self.generated = super::check(lvgl::lv_label_create(self.container))?;
            lvgl::lv_obj_set_width(self.generated, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                self.generated,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );

            self.apply = super::check(lvgl::lv_button_create(self.container))?;
            let label = super::check(lvgl::lv_label_create(self.apply))?;
            lvgl::lv_label_set_text(label, t!(c"general.time_zone.form.apply").as_ptr());
            lvgl::lv_obj_center(label);

            self.update_visibility();
            let _ = self.update_generated();
        }
        Ok(())
    }

    /// # Safety
    /// The widgets must be live and the caller must hold the graphics lock.
    unsafe fn text(text_area: *mut lvgl::lv_obj_t) -> String {
        let text = unsafe { core::ffi::CStr::from_ptr(lvgl::lv_textarea_get_text(text_area)) };
        String::from(text.to_str().unwrap_or("\u{fffd}").trim())
    }

    /// The TZ string of the current values.
    ///
    /// The caller must hold the graphics lock.
    pub fn build(&self) -> core::result::Result<String, FormError> {
        // SAFETY: the widgets are live and the caller holds the graphics lock.
        unsafe {
            let hours = lvgl::lv_dropdown_get_selected(self.hours) as i16 + MINIMUM_OFFSET_HOURS;
            let minutes = OFFSET_MINUTES[lvgl::lv_dropdown_get_selected(self.minutes) as usize % 4];
            let offset_minutes = if hours < 0 {
                hours * 60 - minutes
            } else {
                hours * 60 + minutes
            };
            let daylight_saving =
                lvgl::lv_obj_has_state(self.daylight_saving, lvgl::LV_STATE_CHECKED).then(|| {
                    DaylightSaving {
                        shift_minutes: SHIFTS
                            [lvgl::lv_dropdown_get_selected(self.shift) as usize % 3],
                        rules: match lvgl::lv_dropdown_get_selected(self.preset) {
                            0 => Rules::EuropeanUnion,
                            1 => Rules::UnitedStates,
                            _ => Rules::Custom {
                                start: self.start.read(),
                                end: self.end.read(),
                            },
                        },
                    }
                });
            let standard_name = Self::text(self.standard_name);
            let daylight_name = Self::text(self.daylight_name);
            build(&Values {
                offset_minutes,
                daylight_saving,
                standard_name: &standard_name,
                daylight_name: &daylight_name,
            })
        }
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
        self.update_visibility();
    }

    /// Show the daylight saving and custom rule fields only when they apply.
    fn update_visibility(&mut self) {
        // SAFETY: the widgets are live and the caller holds the graphics lock.
        unsafe {
            let daylight = lvgl::lv_obj_has_state(self.daylight_saving, lvgl::LV_STATE_CHECKED);
            let custom = lvgl::lv_dropdown_get_selected(self.preset) == 2;
            for (object, visible) in [
                (self.daylight_container, daylight),
                (self.custom_container, custom),
            ] {
                if visible {
                    lvgl::lv_obj_remove_flag(object, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                } else {
                    lvgl::lv_obj_add_flag(object, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                }
            }
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
                Err(FormError::InvalidAbbreviation) => lvgl::lv_label_set_text(
                    self.generated,
                    t!(c"general.time_zone.form.errors.abbreviation").as_ptr(),
                ),
                Err(FormError::Invalid) => lvgl::lv_label_set_text(
                    self.generated,
                    t!(c"general.time_zone.errors.invalid").as_ptr(),
                ),
            }
            lvgl::lv_theme_shadcn_set_error(self.generated, result.is_err());
        }
        result
    }

    /// Handle an event of the form. The caller holds the graphics lock.
    pub fn handle_event(&mut self, event: &Event) -> Action {
        if event.code == EventKind::Clicked && event.is_target(self.apply) {
            return Action::Apply(self.build());
        }
        if event.code != EventKind::ValueChanged {
            return Action::None;
        }
        let fields = [
            self.hours,
            self.minutes,
            self.daylight_saving,
            self.shift,
            self.preset,
            self.standard_name,
            self.daylight_name,
        ];
        if !fields.iter().any(|field| event.is_target(*field))
            && !self.start.contains(event)
            && !self.end.contains(event)
        {
            return Action::None;
        }
        self.update_visibility();
        Action::Changed(
            self.update_generated()
                .map(|string| TimeZone::parse(&string).unwrap_or_else(|_| xila::local_time::utc())),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(offset_minutes: i16, daylight_saving: Option<DaylightSaving>) -> Values<'static> {
        Values {
            offset_minutes,
            daylight_saving,
            standard_name: "",
            daylight_name: "",
        }
    }

    const ONE_HOUR: i16 = 60;

    #[test]
    fn fixed_offsets_use_the_posix_sign() {
        assert_eq!(build(&values(60, None)).unwrap(), "<+01>-1");
        assert_eq!(build(&values(330, None)).unwrap(), "<+0530>-5:30");
        assert_eq!(build(&values(345, None)).unwrap(), "<+0545>-5:45");
        assert_eq!(build(&values(-180, None)).unwrap(), "<-03>3");
        assert_eq!(build(&values(-210, None)).unwrap(), "<-0330>3:30");
        assert_eq!(build(&values(0, None)).unwrap(), "<+00>0");
        assert_eq!(build(&values(14 * 60, None)).unwrap(), "<+14>-14");
    }

    #[test]
    fn presets_match_tzdata() {
        let european_union = Some(DaylightSaving {
            shift_minutes: ONE_HOUR,
            rules: Rules::EuropeanUnion,
        });
        let paris = Values {
            standard_name: "CET",
            daylight_name: "CEST",
            ..values(60, european_union)
        };
        assert_eq!(build(&paris).unwrap(), "CET-1CEST,M3.5.0,M10.5.0/3");
        let london = Values {
            standard_name: "GMT",
            daylight_name: "BST",
            ..values(0, european_union)
        };
        assert_eq!(build(&london).unwrap(), "GMT0BST,M3.5.0/1,M10.5.0");
        let athens = Values {
            standard_name: "EET",
            daylight_name: "EEST",
            ..values(120, european_union)
        };
        assert_eq!(build(&athens).unwrap(), "EET-2EEST,M3.5.0/3,M10.5.0/4");
        let azores = values(-60, european_union);
        assert_eq!(build(&azores).unwrap(), "<-01>1<+00>,M3.5.0/0,M10.5.0/1");

        let united_states = Some(DaylightSaving {
            shift_minutes: ONE_HOUR,
            rules: Rules::UnitedStates,
        });
        let new_york = Values {
            standard_name: "EST",
            daylight_name: "EDT",
            ..values(-300, united_states)
        };
        assert_eq!(build(&new_york).unwrap(), "EST5EDT,M3.2.0,M11.1.0");
        let st_johns = Values {
            standard_name: "NST",
            daylight_name: "NDT",
            ..values(-210, united_states)
        };
        assert_eq!(build(&st_johns).unwrap(), "NST3:30NDT,M3.2.0,M11.1.0");
    }

    #[test]
    fn the_built_zone_converts_like_tzdata() {
        let european_union = Some(DaylightSaving {
            shift_minutes: ONE_HOUR,
            rules: Rules::EuropeanUnion,
        });
        let built = TimeZone::parse(&build(&values(60, european_union)).unwrap()).unwrap();
        let paris = TimeZone::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        // Around both 2026 changes: 2026-03-29 01:00 UTC and 2026-10-25 01:00 UTC.
        for utc in [
            1_774_745_999,
            1_774_746_000,
            1_792_890_000 - 1,
            1_792_890_000,
        ] {
            assert_eq!(built.offset_at(utc), paris.offset_at(utc), "{utc}");
        }
        assert_eq!(built.from_utc(1_774_746_000).abbreviation, "+02");
    }

    #[test]
    fn custom_rules_and_shifts() {
        // Southern hemisphere: first Sunday of October to first Sunday of
        // April, with a 30 minute shift (Lord Howe Island).
        let custom = Some(DaylightSaving {
            shift_minutes: 30,
            rules: Rules::Custom {
                start: Transition {
                    month: 10,
                    week: 1,
                    weekday: 0,
                    time_minutes: 120,
                },
                end: Transition {
                    month: 4,
                    week: 1,
                    weekday: 0,
                    time_minutes: 120,
                },
            },
        });
        assert_eq!(
            build(&values(630, custom)).unwrap(),
            "<+1030>-10:30<+11>-11,M10.1.0,M4.1.0"
        );
        let two_hours = Some(DaylightSaving {
            shift_minutes: 120,
            rules: Rules::UnitedStates,
        });
        assert_eq!(
            build(&values(0, two_hours)).unwrap(),
            "<+00>0<+02>-2,M3.2.0,M11.1.0"
        );
    }

    #[test]
    fn abbreviations() {
        let with = |name| Values {
            standard_name: name,
            ..values(60, None)
        };
        assert_eq!(build(&with("CET")).unwrap(), "CET-1");
        assert_eq!(build(&with("UTC1")).unwrap(), "<UTC1>-1");
        for invalid in ["AB", "TOOLONGNAME", "C T", "CÉT"] {
            assert_eq!(
                build(&with(invalid)),
                Err(FormError::InvalidAbbreviation),
                "{invalid}"
            );
        }
    }
}
