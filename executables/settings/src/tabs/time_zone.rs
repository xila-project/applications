//! The time zone section of the General tab.
//!
//! The section shows the stored time zone, a preview of the local time, and one
//! editor at a time, chosen in a dropdown:
//!
//! - with a `time-zone-names-*` feature, the zone selector of
//!   [`super::time_zone_names`] (the default);
//! - the offset form of [`super::time_zone_form`], which builds a TZ string
//!   from an offset and daylight saving rules (the default without names);
//! - a field for a raw POSIX TZ string, with validation.
//!
//! Saving goes through `xila::local_time::set`, which stores the setting and
//! publishes it as `TZ` to new tasks.

use crate::error::Result;
use alloc::string::String;
use core::fmt::Write;
use xila::executable::ExecutableContext;
use xila::graphics::{Event, EventKind, lvgl};
use xila::internationalization::t;
use xila::local_time::{self, Setting, SettingError, TimeZone};
use xila::virtual_file_system::IoErrorKind;

/// Why a setting was not saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SaveError {
    Invalid(SettingError),
    Storage(IoErrorKind),
}

/// Check and store a setting for the current task.
pub(crate) async fn save(
    context: &'static ExecutableContext,
    name: Option<&str>,
    time_zone: &str,
) -> core::result::Result<Setting, SaveError> {
    let setting = Setting::new(name, time_zone).map_err(SaveError::Invalid)?;
    let task = context.task_manager.get_current_task_identifier().await;
    local_time::set(
        context.task_manager,
        context.users_manager,
        context.virtual_file_system,
        task,
        &setting,
    )
    .await
    .map_err(SaveError::Storage)?;
    Ok(setting)
}

/// Write the preview of the local time in `zone`, for example
/// `2026-03-29 03:00:00 CEST (UTC+02:00)`. `utc` is None when the clock is
/// not set.
pub(crate) fn write_preview(output: &mut String, zone: &TimeZone, utc: Option<i64>) {
    output.clear();
    let Some(utc) = utc else {
        let _ = write!(output, "{}", t!("general.time_zone.unsynchronized"));
        return;
    };
    let local = zone.from_utc(utc);
    let date_time = local.date_time;
    let _ = write!(
        output,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {} (UTC",
        date_time.year,
        date_time.month,
        date_time.day,
        date_time.hour,
        date_time.minute,
        date_time.second,
        local.abbreviation,
    );
    let _ = local_time::write_offset(output, local.offset_seconds);
    output.push(')');
}

/// Write the description of the stored setting: its name, or "Custom", and
/// its TZ string.
pub(crate) fn write_current(output: &mut String, setting: &Setting) {
    output.clear();
    match setting.name() {
        Some(name) => output.push_str(name),
        None => {
            let _ = write!(output, "{}", t!("general.time_zone.custom_name"));
        }
    }
    output.push_str(" (");
    output.push_str(setting.time_zone_string());
    output.push(')');
}

/// Set a label to a Rust string. LVGL copies the text.
///
/// # Safety
/// `label` must be a live label and the caller must hold the graphics lock.
unsafe fn set_label(label: *mut lvgl::lv_obj_t, text: &mut String) {
    text.push('\0');
    unsafe { lvgl::lv_label_set_text(label, text.as_ptr().cast()) };
    text.pop();
}

/// The editors of the section, in the order of the editor dropdown. The zone
/// selector exists only with a `time-zone-names-*` feature; then it is the
/// default, otherwise the offset form is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Editor {
    #[cfg(feature = "time-zone-names")]
    Zone,
    Offset,
    String,
}

const EDITORS: &[Editor] = &[
    #[cfg(feature = "time-zone-names")]
    Editor::Zone,
    Editor::Offset,
    Editor::String,
];

pub struct TimeZoneSection {
    context: &'static ExecutableContext,
    current_label: *mut lvgl::lv_obj_t,
    preview_label: *mut lvgl::lv_obj_t,
    editor: *mut lvgl::lv_obj_t,
    form: super::time_zone_form::OffsetForm,
    string_container: *mut lvgl::lv_obj_t,
    #[cfg(feature = "time-zone-names")]
    zone_container: *mut lvgl::lv_obj_t,
    text_area: *mut lvgl::lv_obj_t,
    apply_button: *mut lvgl::lv_obj_t,
    status_label: *mut lvgl::lv_obj_t,
    #[cfg(feature = "time-zone-names")]
    selector: super::time_zone_names::ZoneSelector,
    /// The zone that the preview shows: the stored one, or the one being
    /// edited once it is valid.
    preview: TimeZone,
    /// The UTC second of the last preview, to update it once per second.
    last_preview: Option<Option<i64>>,
    text: String,
}

impl TimeZoneSection {
    pub fn new(context: &'static ExecutableContext) -> Self {
        Self {
            context,
            current_label: core::ptr::null_mut(),
            preview_label: core::ptr::null_mut(),
            editor: core::ptr::null_mut(),
            form: super::time_zone_form::OffsetForm::new(),
            string_container: core::ptr::null_mut(),
            #[cfg(feature = "time-zone-names")]
            zone_container: core::ptr::null_mut(),
            text_area: core::ptr::null_mut(),
            apply_button: core::ptr::null_mut(),
            status_label: core::ptr::null_mut(),
            #[cfg(feature = "time-zone-names")]
            selector: super::time_zone_names::ZoneSelector::new(),
            preview: local_time::utc(),
            last_preview: None,
            text: String::new(),
        }
    }

    /// Build the section in `parent`.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    pub async unsafe fn create_ui(&mut self, parent: *mut lvgl::lv_obj_t) -> Result<()> {
        let task = self
            .context
            .task_manager
            .get_current_task_identifier()
            .await;
        let setting = local_time::load(self.context.virtual_file_system, task).await;
        self.preview = setting.time_zone();

        unsafe {
            let card = super::card(parent)?;
            lvgl::lv_obj_set_style_pad_row(card, 6, 0);
            let title = super::check(lvgl::lv_label_create(card))?;
            lvgl::lv_label_set_text(title, t!(c"general.time_zone.title").as_ptr());

            self.current_label = super::check(lvgl::lv_label_create(card))?;
            lvgl::lv_obj_set_width(self.current_label, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                self.current_label,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );
            write_current(&mut self.text, &setting);
            set_label(self.current_label, &mut self.text);

            super::caption(card, t!(c"general.time_zone.preview"))?;
            self.preview_label = super::check(lvgl::lv_label_create(card))?;
            lvgl::lv_obj_set_width(self.preview_label, lvgl::lv_pct(100));

            let mut editors = String::new();
            for editor in EDITORS {
                if !editors.is_empty() {
                    editors.push('\n');
                }
                editors.push_str(match editor {
                    #[cfg(feature = "time-zone-names")]
                    Editor::Zone => t!("general.time_zone.editors.zone"),
                    Editor::Offset => t!("general.time_zone.editors.offset"),
                    Editor::String => t!("general.time_zone.editors.string"),
                });
            }
            editors.push('\0');
            super::caption(card, t!(c"general.time_zone.editor"))?;
            self.editor = super::check(lvgl::lv_dropdown_create(card))?;
            lvgl::lv_obj_set_width(self.editor, lvgl::lv_pct(100));
            lvgl::lv_dropdown_set_options(self.editor, editors.as_ptr().cast());

            let column = |parent| -> Result<*mut lvgl::lv_obj_t> {
                let column = super::check(lvgl::lv_obj_create(parent))?;
                lvgl::lv_obj_remove_style_all(column);
                lvgl::lv_obj_set_size(column, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
                lvgl::lv_obj_set_flex_flow(column, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
                lvgl::lv_obj_set_style_pad_row(column, 6, 0);
                Ok(column)
            };

            #[cfg(feature = "time-zone-names")]
            {
                self.zone_container = column(card)?;
                self.selector
                    .create_ui(self.zone_container, setting.name())?;
            }

            self.form.create_ui(card)?;

            self.string_container = column(card)?;
            let field = self.string_container;
            super::caption(field, t!(c"general.time_zone.custom"))?;
            self.text_area = super::check(lvgl::lv_textarea_create(field))?;
            lvgl::lv_textarea_set_one_line(self.text_area, true);
            lvgl::lv_textarea_set_max_length(
                self.text_area,
                local_time::MAXIMUM_TIME_ZONE_LENGTH as u32,
            );
            lvgl::lv_textarea_set_placeholder_text(
                self.text_area,
                c"CET-1CEST,M3.5.0,M10.5.0/3".as_ptr(),
            );
            lvgl::lv_obj_set_width(self.text_area, lvgl::lv_pct(100));
            super::caption(field, t!(c"general.time_zone.custom_hint"))?;

            self.apply_button = super::check(lvgl::lv_button_create(field))?;
            let label = super::check(lvgl::lv_label_create(self.apply_button))?;
            lvgl::lv_label_set_text(label, t!(c"general.time_zone.apply_custom").as_ptr());
            lvgl::lv_obj_center(label);

            self.status_label = super::check(lvgl::lv_label_create(card))?;
            lvgl::lv_label_set_text(self.status_label, c"".as_ptr());
            lvgl::lv_obj_set_width(self.status_label, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                self.status_label,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );
        }
        self.show_editor();
        self.refresh();
        Ok(())
    }

    fn selected_editor(&self) -> Editor {
        // SAFETY: the dropdown is live and the caller holds the graphics lock.
        let index = unsafe { lvgl::lv_dropdown_get_selected(self.editor) } as usize;
        EDITORS.get(index).copied().unwrap_or(EDITORS[0])
    }

    /// Show the selected editor only.
    fn show_editor(&mut self) {
        let selected = self.selected_editor();
        let containers = [
            #[cfg(feature = "time-zone-names")]
            (Editor::Zone, self.zone_container),
            (Editor::Offset, self.form.container()),
            (Editor::String, self.string_container),
        ];
        for (editor, container) in containers {
            // SAFETY: the containers are live and the caller holds the
            // graphics lock.
            unsafe {
                if editor == selected {
                    lvgl::lv_obj_remove_flag(container, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                } else {
                    lvgl::lv_obj_add_flag(container, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                }
            }
        }
    }

    /// Update the preview once per second.
    ///
    /// The caller must hold the graphics lock.
    pub fn refresh(&mut self) {
        if self.preview_label.is_null() {
            return;
        }
        let utc = self
            .context
            .time_manager
            .realtime_now()
            .and_then(|utc| utc.try_as_seconds())
            .ok();
        if self.last_preview == Some(utc) {
            return;
        }
        self.last_preview = Some(utc);
        write_preview(&mut self.text, &self.preview, utc);
        // SAFETY: the label is live and the caller holds the graphics lock.
        unsafe { set_label(self.preview_label, &mut self.text) };
    }

    fn show_status(&mut self, message: &core::ffi::CStr, is_error: bool) {
        // SAFETY: the label is live and the caller holds the graphics lock.
        unsafe {
            lvgl::lv_label_set_text(self.status_label, message.as_ptr());
            lvgl::lv_theme_shadcn_set_error(self.status_label, is_error);
        }
    }

    fn show_error(&mut self, error: SaveError) {
        let message = match error {
            SaveError::Invalid(SettingError::TimeZoneTooLong) => {
                t!(c"general.time_zone.errors.too_long")
            }
            SaveError::Invalid(_) => t!(c"general.time_zone.errors.invalid"),
            SaveError::Storage(IoErrorKind::PermissionDenied) => {
                t!(c"general.time_zone.errors.permission_denied")
            }
            SaveError::Storage(_) => t!(c"general.time_zone.errors.save_failed"),
        };
        self.show_status(message, true);
    }

    fn set_preview(&mut self, zone: TimeZone) {
        self.preview = zone;
        self.last_preview = None;
        self.refresh();
    }

    /// The text of the custom field, without surrounding spaces.
    fn custom_text(&self) -> String {
        // SAFETY: the text area is live and the caller holds the graphics lock.
        let text = unsafe { core::ffi::CStr::from_ptr(lvgl::lv_textarea_get_text(self.text_area)) };
        String::from(text.to_str().unwrap_or("\u{fffd}").trim())
    }

    async fn apply(&mut self, name: Option<&str>, time_zone: &str) {
        match save(self.context, name, time_zone).await {
            Ok(setting) => {
                write_current(&mut self.text, &setting);
                // SAFETY: the label is live and the caller holds the graphics lock.
                unsafe { set_label(self.current_label, &mut self.text) };
                self.set_preview(setting.time_zone());
                self.show_status(t!(c"general.time_zone.saved"), false);
            }
            Err(error) => self.show_error(error),
        }
    }

    /// Handle an event of the section. The caller holds the graphics lock.
    pub async fn handle_event(&mut self, event: &Event) -> bool {
        if event.code == EventKind::ValueChanged && event.is_target(self.editor) {
            self.show_editor();
            self.show_status(c"", false);
            return true;
        }
        match self.form.handle_event(event) {
            super::time_zone_form::Action::None => {}
            super::time_zone_form::Action::Changed(Ok(zone)) => {
                self.set_preview(zone);
                return true;
            }
            super::time_zone_form::Action::Changed(Err(_)) => return true,
            super::time_zone_form::Action::Apply(Ok(string)) => {
                self.apply(None, &string).await;
                return true;
            }
            super::time_zone_form::Action::Apply(Err(_)) => {
                self.show_status(t!(c"general.time_zone.errors.invalid"), true);
                return true;
            }
        }
        if event.code == EventKind::ValueChanged && event.is_target(self.text_area) {
            let text = self.custom_text();
            if text.is_empty() {
                self.show_status(c"", false);
            } else {
                match local_time::validate_time_zone(&text) {
                    Ok(zone) => {
                        self.show_status(c"", false);
                        self.set_preview(zone);
                    }
                    Err(error) => self.show_error(SaveError::Invalid(error)),
                }
            }
            return true;
        }
        if event.code == EventKind::Clicked && event.is_target(self.apply_button) {
            let text = self.custom_text();
            self.apply(None, &text).await;
            return true;
        }
        #[cfg(feature = "time-zone-names")]
        match self.selector.handle_event(event) {
            super::time_zone_names::Action::None => {}
            super::time_zone_names::Action::Handled => return true,
            super::time_zone_names::Action::Preview(zone) => {
                self.set_preview(zone);
                return true;
            }
            super::time_zone_names::Action::Apply { name, time_zone } => {
                self.apply(Some(name), time_zone).await;
                return true;
            }
        }
        false
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;

    const PARIS: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

    #[test]
    fn preview_at_a_daylight_saving_change() {
        let paris = TimeZone::parse(PARIS).unwrap();
        let mut output = String::new();
        // 2026-03-29 00:59:59 UTC is the last second of winter time.
        write_preview(&mut output, &paris, Some(1_774_745_999));
        assert_eq!(output, "2026-03-29 01:59:59 CET (UTC+01:00)");
        write_preview(&mut output, &paris, Some(1_774_746_000));
        assert_eq!(output, "2026-03-29 03:00:00 CEST (UTC+02:00)");
        let kathmandu = TimeZone::parse("<+0545>-5:45").unwrap();
        write_preview(&mut output, &kathmandu, Some(0));
        assert_eq!(output, "1970-01-01 05:45:00 +0545 (UTC+05:45)");
    }

    /// Exercise the section through the real widgets: an invalid string is
    /// refused, a valid one is saved and published.
    pub(crate) async fn exercise_ui(
        context: &'static ExecutableContext,
        graphics: &'static xila::graphics::Manager,
    ) {
        let task = context.task_manager.get_current_task_identifier().await;
        let _lock = graphics.lock().await;
        let screen = unsafe { lvgl::lv_screen_active() };
        let mut section = TimeZoneSection::new(context);
        unsafe { section.create_ui(screen).await.unwrap() };

        // `UTC` has no offset, so it is not a valid TZ string.
        unsafe { lvgl::lv_textarea_set_text(section.text_area, c"UTC".as_ptr()) };
        let text = section.custom_text();
        section.apply(None, &text).await;
        assert!(
            local_time::read(context.virtual_file_system, task)
                .await
                .is_err()
        );

        unsafe { lvgl::lv_textarea_set_text(section.text_area, c" JST-9 ".as_ptr()) };
        let text = section.custom_text();
        section.apply(None, &text).await;
        let stored = local_time::read(context.virtual_file_system, task)
            .await
            .unwrap();
        assert_eq!(stored.name(), None);
        assert_eq!(stored.time_zone_string(), "JST-9");
        assert_eq!(
            context
                .task_manager
                .get_environment_variable(task, local_time::ENVIRONMENT_VARIABLE)
                .await
                .unwrap()
                .get_value(),
            "JST-9"
        );
        let current =
            unsafe { core::ffi::CStr::from_ptr(lvgl::lv_label_get_text(section.current_label)) };
        assert!(current.to_str().unwrap().ends_with("(JST-9)"));

        // The offset form: UTC+01:00 with the European Union rules (the
        // default preset) and no abbreviations.
        assert_eq!(section.form.build().unwrap(), "<+00>0");
        unsafe { section.form.select(1, true) };
        let string = section.form.build().unwrap();
        assert_eq!(string, "<+01>-1<+02>,M3.5.0,M10.5.0/3");
        section.apply(None, &string).await;
        assert_eq!(
            local_time::read(context.virtual_file_system, task)
                .await
                .unwrap()
                .time_zone_string(),
            string
        );

        #[cfg(any(feature = "time-zone-names-europe", feature = "time-zone-names-all"))]
        {
            section
                .apply(
                    Some("Europe/Paris"),
                    xila::time_zone::iana::time_zone_string("Europe/Paris").unwrap(),
                )
                .await;
            let stored = local_time::read(context.virtual_file_system, task)
                .await
                .unwrap();
            assert_eq!(stored.name(), Some("Europe/Paris"));
            assert_eq!(stored.time_zone_string(), PARIS);
        }

        unsafe { lvgl::lv_obj_clean(screen) };
    }
}
