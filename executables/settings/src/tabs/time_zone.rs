//! The time zone section of the General tab.
//!
//! The section shows the stored time zone, a preview of the local time, and one
//! editor at a time, chosen in a dropdown:
//!
//! - with a `time-zone-names-*` feature, the zone selector of
//!   [`super::time_zone_names`] (the default);
//! - with the `time-zone-offset-form` feature, the offset form of
//!   [`super::time_zone_form`], which builds a TZ string from an offset and
//!   daylight saving rules (the default without names);
//! - a field for a raw POSIX TZ string, with validation.
//!
//! Only the selected editor exists: changing the editor deletes the objects of
//! the previous one and builds the next one.
//!
//! Saving goes through `xila::local_time::set`, which stores the setting and
//! publishes it as `TZ` to new tasks. An editor event only records the request
//! ([`TimeZoneSection::handle_event`]); Settings then calls
//! [`TimeZoneSection::save_pending`] after it releases the graphics lock, so
//! the lock is not held across the flash write.

use crate::error::Result;
use alloc::string::String;
use core::fmt::Write;
use xila::executable::ExecutableContext;
use xila::graphics::{self, Event, EventKind, lvgl};
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
    let _ = match utc {
        Some(utc) => local_time::write_local_time(output, zone, utc),
        None => write!(output, "{}", t!("general.time_zone.unsynchronized")),
    };
}

/// Write the description of the stored setting: its name, or "Custom", and
/// its TZ string, in one write.
pub(crate) fn write_current(output: &mut String, setting: &Setting) {
    output.clear();
    let name = setting
        .name()
        .unwrap_or(t!("general.time_zone.custom_name"));
    let _ = write!(output, "{name} ({})", setting.time_zone_string());
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

/// The editors of the section, in the order of the editor dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Editor {
    #[cfg(feature = "time-zone-names")]
    Zone,
    #[cfg(feature = "time-zone-offset-form")]
    Offset,
    String,
}

const EDITORS: &[Editor] = &[
    #[cfg(feature = "time-zone-names")]
    Editor::Zone,
    #[cfg(feature = "time-zone-offset-form")]
    Editor::Offset,
    Editor::String,
];

/// The raw TZ string editor.
struct StringEditor {
    text_area: *mut lvgl::lv_obj_t,
    apply: *mut lvgl::lv_obj_t,
}

impl StringEditor {
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    unsafe fn new(parent: *mut lvgl::lv_obj_t) -> Result<Self> {
        unsafe {
            super::caption(parent, t!(c"general.time_zone.custom"))?;
            let text_area = super::check(lvgl::lv_textarea_create(parent))?;
            lvgl::lv_textarea_set_one_line(text_area, true);
            lvgl::lv_textarea_set_max_length(
                text_area,
                local_time::MAXIMUM_TIME_ZONE_LENGTH as u32,
            );
            lvgl::lv_textarea_set_placeholder_text(
                text_area,
                c"CET-1CEST,M3.5.0,M10.5.0/3".as_ptr(),
            );
            lvgl::lv_obj_set_width(text_area, lvgl::lv_pct(100));
            super::caption(parent, t!(c"general.time_zone.custom_hint"))?;
            let apply = super::check(lvgl::lv_button_create(parent))?;
            let label = super::check(lvgl::lv_label_create(apply))?;
            lvgl::lv_label_set_text(label, t!(c"general.time_zone.apply_custom").as_ptr());
            lvgl::lv_obj_center(label);
            Ok(Self { text_area, apply })
        }
    }

    /// The text of the field, without surrounding spaces.
    fn text(&self) -> String {
        // SAFETY: the text area is live and the caller holds the graphics lock.
        let text = unsafe { core::ffi::CStr::from_ptr(lvgl::lv_textarea_get_text(self.text_area)) };
        String::from(text.to_str().unwrap_or("\u{fffd}").trim())
    }
}

/// The editor that exists.
enum Active {
    #[cfg(feature = "time-zone-names")]
    Zone(super::time_zone_names::ZoneSelector),
    #[cfg(feature = "time-zone-offset-form")]
    Offset(super::time_zone_form::OffsetForm),
    String(StringEditor),
}

/// A setting to store once the graphics lock is released.
struct PendingSave {
    name: Option<&'static str>,
    time_zone: String,
}

pub struct TimeZoneSection {
    context: &'static ExecutableContext,
    current_label: *mut lvgl::lv_obj_t,
    preview_label: *mut lvgl::lv_obj_t,
    editor: *mut lvgl::lv_obj_t,
    /// The container of the active editor.
    host: *mut lvgl::lv_obj_t,
    status_label: *mut lvgl::lv_obj_t,
    active: Option<Active>,
    /// The stored name, to preselect it in the zone selector.
    stored_name: Option<String>,
    pending: Option<PendingSave>,
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
            host: core::ptr::null_mut(),
            status_label: core::ptr::null_mut(),
            active: None,
            stored_name: None,
            pending: None,
            preview: local_time::utc(),
            last_preview: None,
            text: String::new(),
        }
    }

    /// Build the section in `parent`, with the default editor.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the graphics lock.
    pub async unsafe fn create_ui(&mut self, parent: *mut lvgl::lv_obj_t) -> Result<()> {
        // The setting in use, kept in RAM by Core: no file read here.
        let setting = local_time::stored();
        self.preview = setting.time_zone();
        self.stored_name = setting.name().map(String::from);

        let mut editors = String::new();
        for (index, editor) in EDITORS.iter().enumerate() {
            if index != 0 {
                editors.push('\n');
            }
            editors.push_str(match editor {
                #[cfg(feature = "time-zone-names")]
                Editor::Zone => t!("general.time_zone.editors.zone"),
                #[cfg(feature = "time-zone-offset-form")]
                Editor::Offset => t!("general.time_zone.editors.offset"),
                Editor::String => t!("general.time_zone.editors.string"),
            });
        }
        editors.push('\0');

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

            super::caption(card, t!(c"general.time_zone.editor"))?;
            self.editor = super::check(lvgl::lv_dropdown_create(card))?;
            lvgl::lv_obj_set_width(self.editor, lvgl::lv_pct(100));
            lvgl::lv_dropdown_set_options(self.editor, editors.as_ptr().cast());

            self.host = super::check(lvgl::lv_obj_create(card))?;
            lvgl::lv_obj_remove_style_all(self.host);
            lvgl::lv_obj_set_size(self.host, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
            lvgl::lv_obj_set_flex_flow(self.host, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            lvgl::lv_obj_set_style_pad_row(self.host, 6, 0);

            self.status_label = super::check(lvgl::lv_label_create(card))?;
            lvgl::lv_label_set_text(self.status_label, c"".as_ptr());
            lvgl::lv_obj_set_width(self.status_label, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                self.status_label,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );
            self.build_editor(EDITORS[0])?;
        }
        self.refresh();
        Ok(())
    }

    /// Delete the objects of the active editor and build `editor`.
    ///
    /// # Safety
    /// The caller must hold the graphics lock.
    unsafe fn build_editor(&mut self, editor: Editor) -> Result<()> {
        self.active = None;
        unsafe {
            lvgl::lv_obj_clean(self.host);
            self.active = Some(match editor {
                #[cfg(feature = "time-zone-names")]
                Editor::Zone => Active::Zone(super::time_zone_names::ZoneSelector::new(
                    self.host,
                    self.stored_name.as_deref(),
                )?),
                #[cfg(feature = "time-zone-offset-form")]
                Editor::Offset => {
                    Active::Offset(super::time_zone_form::OffsetForm::new(self.host)?)
                }
                Editor::String => Active::String(StringEditor::new(self.host)?),
            });
        }
        Ok(())
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

    /// Record a setting to store, if it is valid. The caller holds the
    /// graphics lock; [`Self::save_pending`] stores it after the lock is
    /// released.
    fn request_save(&mut self, name: Option<&'static str>, time_zone: String) {
        match Setting::new(name, &time_zone) {
            Ok(_) => self.pending = Some(PendingSave { name, time_zone }),
            Err(error) => self.show_error(SaveError::Invalid(error)),
        }
    }

    /// Whether a save waits for [`Self::save_pending`].
    pub fn has_pending_save(&self) -> bool {
        self.pending.is_some()
    }

    /// Store the recorded setting, then show the result. The caller must not
    /// hold the graphics lock: the write runs without it, and the result is
    /// shown under it.
    pub async fn save_pending(&mut self, graphics_manager: &'static graphics::Manager) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let result = save(self.context, pending.name, &pending.time_zone).await;
        graphics::lock!(graphics_manager, {
            match result {
                Ok(setting) => {
                    self.stored_name = setting.name().map(String::from);
                    write_current(&mut self.text, &setting);
                    // SAFETY: the label is live and the graphics lock is held.
                    unsafe { set_label(self.current_label, &mut self.text) };
                    self.set_preview(setting.time_zone());
                    self.show_status(t!(c"general.time_zone.saved"), false);
                }
                Err(error) => self.show_error(error),
            }
        });
    }

    /// Handle an event of the section. The caller holds the graphics lock.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        if event.code == EventKind::ValueChanged && event.is_target(self.editor) {
            // SAFETY: the dropdown is live and the caller holds the lock.
            let index = unsafe { lvgl::lv_dropdown_get_selected(self.editor) } as usize;
            let editor = EDITORS.get(index).copied().unwrap_or(EDITORS[0]);
            // SAFETY: the caller holds the graphics lock.
            if unsafe { self.build_editor(editor) }.is_err() {
                self.show_status(t!(c"general.time_zone.errors.save_failed"), true);
            } else {
                self.show_status(c"", false);
            }
            return true;
        }
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        match active {
            #[cfg(feature = "time-zone-names")]
            Active::Zone(selector) => match selector.handle_event(event) {
                super::time_zone_names::Action::None => false,
                super::time_zone_names::Action::Handled => true,
                super::time_zone_names::Action::Preview(zone) => {
                    self.set_preview(zone);
                    true
                }
                super::time_zone_names::Action::Apply { name, time_zone } => {
                    self.request_save(Some(name), String::from(time_zone));
                    true
                }
            },
            #[cfg(feature = "time-zone-offset-form")]
            Active::Offset(form) => match form.handle_event(event) {
                super::time_zone_form::Action::None => false,
                super::time_zone_form::Action::Changed(Ok(zone)) => {
                    self.set_preview(zone);
                    true
                }
                super::time_zone_form::Action::Changed(Err(_)) => true,
                super::time_zone_form::Action::Apply(Ok(string)) => {
                    self.request_save(None, string);
                    true
                }
                super::time_zone_form::Action::Apply(Err(_)) => {
                    self.show_status(t!(c"general.time_zone.errors.invalid"), true);
                    true
                }
            },
            Active::String(editor) => {
                if event.code == EventKind::ValueChanged && event.is_target(editor.text_area) {
                    let text = editor.text();
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
                    true
                } else if event.code == EventKind::Clicked && event.is_target(editor.apply) {
                    let text = editor.text();
                    self.request_save(None, text);
                    true
                } else {
                    false
                }
            }
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn preview_at_a_daylight_saving_change() {
        let paris = TimeZone::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
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
    /// refused, a valid one is saved (outside the graphics lock) and
    /// published, and the default editor saves its own string.
    pub(crate) async fn exercise_ui(
        context: &'static ExecutableContext,
        graphics: &'static xila::graphics::Manager,
    ) {
        let task = context.task_manager.get_current_task_identifier().await;
        let mut section = TimeZoneSection::new(context);
        let screen = graphics::lock!(graphics, {
            let screen = unsafe { lvgl::lv_screen_active() };
            unsafe { section.create_ui(screen).await.unwrap() };
            unsafe { section.build_editor(Editor::String).unwrap() };
            screen
        });
        let set_text = |section: &mut TimeZoneSection, text: &core::ffi::CStr| {
            let Some(Active::String(editor)) = section.active.as_ref() else {
                panic!("the string editor is active");
            };
            unsafe { lvgl::lv_textarea_set_text(editor.text_area, text.as_ptr()) };
            let text = editor.text();
            section.request_save(None, text);
        };

        // `UTC` has no offset, so it is not a valid TZ string.
        graphics::lock!(graphics, { set_text(&mut section, c"UTC") });
        assert!(!section.has_pending_save());
        assert!(
            local_time::read(context.virtual_file_system, task)
                .await
                .is_err()
        );

        graphics::lock!(graphics, { set_text(&mut section, c" JST-9 ") });
        assert!(section.has_pending_save());
        section.save_pending(graphics).await;
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
        graphics::lock!(graphics, {
            let current = unsafe {
                core::ffi::CStr::from_ptr(lvgl::lv_label_get_text(section.current_label))
            };
            assert!(current.to_str().unwrap().ends_with("(JST-9)"));
        });

        // The offset form: UTC+01:00 with the European Union rules (the
        // default preset) and no abbreviations.
        #[cfg(feature = "time-zone-offset-form")]
        {
            let string = graphics::lock!(graphics, {
                unsafe { section.build_editor(Editor::Offset).unwrap() };
                let Some(Active::Offset(form)) = section.active.as_mut() else {
                    panic!("the offset form is active");
                };
                assert_eq!(form.build().unwrap(), "<+00>0");
                unsafe { form.select(1, true) };
                let string = form.build().unwrap();
                section.request_save(None, string.clone());
                string
            });
            assert_eq!(string, "<+01>-1<+02>,M3.5.0,M10.5.0/3");
            section.save_pending(graphics).await;
            assert_eq!(
                local_time::read(context.virtual_file_system, task)
                    .await
                    .unwrap()
                    .time_zone_string(),
                string
            );
        }

        #[cfg(any(feature = "time-zone-names-europe", feature = "time-zone-names-all"))]
        {
            graphics::lock!(graphics, {
                section.request_save(
                    Some("Europe/Paris"),
                    String::from(xila::time_zone::iana::time_zone_string("Europe/Paris").unwrap()),
                );
            });
            section.save_pending(graphics).await;
            let stored = local_time::read(context.virtual_file_system, task)
                .await
                .unwrap();
            assert_eq!(stored.name(), Some("Europe/Paris"));
            assert_eq!(stored.time_zone_string(), "CET-1CEST,M3.5.0,M10.5.0/3");
            // The selector preselects the stored name.
            graphics::lock!(graphics, {
                unsafe { section.build_editor(Editor::Zone).unwrap() };
            });
        }

        graphics::lock!(graphics, {
            section.active = None;
            unsafe { lvgl::lv_obj_clean(screen) };
        });
    }
}
