use crate::error::{Error, Result};
use alloc::ffi::CString;
use core::{ffi::CStr, ptr::null_mut};
use xila::graphics::appearance::{Appearance, MINIMUM_BRIGHTNESS};
use xila::{
    executable::ExecutableContext,
    graphics::{self, Event, EventKind, lvgl},
    internationalization::t,
};

pub struct AppearanceTab {
    context: &'static ExecutableContext,
    appearance: Appearance,
    load_failed: bool,
    dark_mode: *mut lvgl::lv_obj_t,
    primary: *mut lvgl::lv_obj_t,
    secondary: *mut lvgl::lv_obj_t,
    /// Present only when the screen has brightness control.
    brightness: *mut lvgl::lv_obj_t,
    brightness_value: *mut lvgl::lv_obj_t,
    has_brightness: bool,
    /// Whether the brightness on screen is the saved one.
    brightness_saved: bool,
    status: *mut lvgl::lv_obj_t,
}

/// Lowest slider position, in percent: the slider can dim the screen but
/// never turn it dark.
const MINIMUM_BRIGHTNESS_PERCENT: u8 = 10;

const _: () = assert!(percent_to_brightness(MINIMUM_BRIGHTNESS_PERCENT) >= MINIMUM_BRIGHTNESS);

/// Converts a slider position, in percent, to a screen brightness level.
const fn percent_to_brightness(percent: u8) -> u8 {
    ((percent as u16 * 255 + 50) / 100) as u8
}

/// Converts a screen brightness level to the nearest slider position.
const fn brightness_to_percent(level: u8) -> u8 {
    ((level as u16 * 100 + 127) / 255) as u8
}

impl AppearanceTab {
    pub async fn new(context: &'static ExecutableContext) -> Self {
        let task = context.task_manager.get_current_task_identifier().await;
        let loaded = Appearance::load(context.virtual_file_system, task).await;
        if let Err(error) = &loaded {
            xila::log::warning!("Could not read appearance preferences: {}", error);
        }
        // Brightness only talks to the screen device, so this works while
        // Settings holds the LVGL lock.
        let has_brightness = graphics::ffi_manager().get_brightness().await.is_ok();
        Self {
            context,
            load_failed: loaded.is_err(),
            appearance: loaded.unwrap_or_default(),
            dark_mode: null_mut(),
            primary: null_mut(),
            secondary: null_mut(),
            brightness: null_mut(),
            brightness_value: null_mut(),
            has_brightness,
            brightness_saved: true,
            status: null_mut(),
        }
    }

    pub async fn create_ui(&mut self, parent: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t> {
        unsafe {
            let tab = check(lvgl::lv_tabview_add_tab(
                parent,
                t!(c"tabs.appearance").as_ptr(),
            ))?;
            super::setup_page(tab);

            // Every setting is one row: its name, then its control.
            let row = super::setting_row(tab, t!(c"appearance.fields.dark_mode"))?;
            self.dark_mode = check(lvgl::lv_switch_create(row))?;

            // LVGL copies this options string; no per-preset objects are needed.
            let options = CString::new(alloc::format!(
                "{}\n{}\n{}\n{}\n{}\n{}",
                t!("appearance.colors.zinc_auto"),
                t!("appearance.colors.blue"),
                t!("appearance.colors.green"),
                t!("appearance.colors.orange"),
                t!("appearance.colors.rose"),
                t!("appearance.colors.violet")
            ))?;
            let width = if super::is_compact() { 150 } else { 220 };
            let row = super::setting_row(tab, t!(c"appearance.fields.primary_color"))?;
            self.primary = check(lvgl::lv_dropdown_create(row))?;
            lvgl::lv_dropdown_set_options(self.primary, options.as_ptr());
            lvgl::lv_obj_set_width(self.primary, width);
            let row = super::setting_row(tab, t!(c"appearance.fields.secondary_color"))?;
            self.secondary = check(lvgl::lv_dropdown_create(row))?;
            lvgl::lv_dropdown_set_options(self.secondary, options.as_ptr());
            lvgl::lv_obj_set_width(self.secondary, width);
            if self.has_brightness {
                let row = super::setting_row(tab, t!(c"appearance.fields.brightness"))?;
                self.brightness = check(lvgl::lv_slider_create(row))?;
                lvgl::lv_slider_set_range(
                    self.brightness,
                    i32::from(MINIMUM_BRIGHTNESS_PERCENT),
                    100,
                );
                // Leave room for the value label within the dropdowns' width.
                lvgl::lv_obj_set_width(self.brightness, width - 56);
                self.brightness_value = create_label(row, c"")?;
                lvgl::lv_obj_set_width(self.brightness_value, 48);
                lvgl::lv_obj_set_style_text_align(
                    self.brightness_value,
                    lvgl::lv_text_align_t_LV_TEXT_ALIGN_RIGHT,
                    0,
                );
            }
            self.sync_controls();
            self.status = create_label(tab, c"")?;
            lvgl::lv_obj_set_width(self.status, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                self.status,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );
            if self.load_failed {
                self.set_status(
                    t!(c"errors.could_not_load_appearance_choose_settings_to_save_a_replacement"),
                    true,
                );
            }
            Ok(tab)
        }
    }

    /// Settings dispatches events while holding the LVGL lock; the guard is
    /// the session source for the theme change.
    pub async fn handle_event(
        &mut self,
        event: &Event,
        guard: &mut graphics::GraphicsGuard<'_>,
    ) -> bool {
        if !self.brightness.is_null() && event.is_target(self.brightness) {
            // A press that is lost, for example to a scroll of the tab, ends
            // like a release: the level on screen must not stay unsaved.
            if matches!(
                event.code,
                EventKind::ValueChanged | EventKind::Released | EventKind::PressLost
            ) {
                self.change_brightness().await;
                return true;
            }
            return false;
        }
        if event.code != EventKind::ValueChanged
            || ![self.dark_mode, self.primary, self.secondary]
                .into_iter()
                .any(|widget| event.is_target(widget))
        {
            return false;
        }
        let appearance = unsafe {
            Appearance {
                is_dark: lvgl::lv_obj_has_state(self.dark_mode, lvgl::LV_STATE_CHECKED),
                primary: lvgl::lv_dropdown_get_selected(self.primary) as u8,
                secondary: lvgl::lv_dropdown_get_selected(self.secondary) as u8,
                ..self.appearance
            }
        };
        if let Err(error) = appearance.apply(graphics::ffi_manager(), guard).await {
            xila::log::warning!("Could not apply appearance: {}", error);
            unsafe {
                self.sync_controls();
                self.set_status(t!(c"errors.could_not_apply_appearance"), true);
            }
            return true;
        }
        self.appearance = appearance;
        self.save().await;
        true
    }

    /// Applies the slider's brightness at once, while it moves, and saves it
    /// when it settles: after a drag, or after each keypad step.
    async fn change_brightness(&mut self) {
        let (level, settled) = unsafe {
            let percent = lvgl::lv_slider_get_value(self.brightness)
                .clamp(i32::from(MINIMUM_BRIGHTNESS_PERCENT), 100) as u8;
            self.show_brightness(percent);
            (
                percent_to_brightness(percent),
                !lvgl::lv_obj_has_state(self.brightness, lvgl::LV_STATE_PRESSED),
            )
        };
        if level != self.appearance.brightness {
            let appearance = Appearance {
                brightness: level,
                ..self.appearance
            };
            if let Err(error) = appearance.apply_brightness(graphics::ffi_manager()).await {
                xila::log::warning!("Could not apply brightness: {}", error);
                unsafe {
                    self.sync_controls();
                    self.set_status(t!(c"errors.could_not_apply_appearance"), true);
                }
                return;
            }
            self.appearance = appearance;
            self.brightness_saved = false;
        }
        if settled && !self.brightness_saved {
            self.brightness_saved = true;
            self.save().await;
        }
    }

    /// Saves the current appearance and reports the outcome.
    async fn save(&self) {
        let task = self
            .context
            .task_manager
            .get_current_task_identifier()
            .await;
        match self
            .appearance
            .save(self.context.virtual_file_system, task)
            .await
        {
            Ok(()) => unsafe {
                self.set_status(t!(c"appearance.status.saved"), false);
            },
            Err(error) => {
                xila::log::warning!("Could not save appearance preferences: {}", error);
                unsafe {
                    self.set_status(t!(c"appearance.status.applied_not_saved"), true);
                }
            }
        }
    }

    unsafe fn show_brightness(&self, percent: u8) {
        unsafe {
            let text = alloc::format!("{percent} %");
            if let Ok(text) = CString::new(text) {
                lvgl::lv_label_set_text(self.brightness_value, text.as_ptr());
            }
        }
    }

    unsafe fn sync_controls(&self) {
        unsafe {
            lvgl::lv_obj_set_state(
                self.dark_mode,
                lvgl::LV_STATE_CHECKED,
                self.appearance.is_dark,
            );
            lvgl::lv_dropdown_set_selected(self.primary, u32::from(self.appearance.primary));
            lvgl::lv_dropdown_set_selected(self.secondary, u32::from(self.appearance.secondary));
            if !self.brightness.is_null() {
                let percent = brightness_to_percent(self.appearance.brightness)
                    .max(MINIMUM_BRIGHTNESS_PERCENT);
                lvgl::lv_slider_set_value(self.brightness, i32::from(percent), false);
                self.show_brightness(percent);
            }
        }
    }

    unsafe fn set_status(&self, message: &CStr, error: bool) {
        unsafe {
            lvgl::lv_label_set_text(self.status, message.as_ptr());
            lvgl::lv_theme_shadcn_set_error(self.status, error);
        }
    }
}

fn check(object: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t> {
    if object.is_null() {
        Err(Error::FailedToCreateUiElement)
    } else {
        Ok(object)
    }
}

unsafe fn create_label(parent: *mut lvgl::lv_obj_t, text: &CStr) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let label = check(lvgl::lv_label_create(parent))?;
        lvgl::lv_label_set_text(label, text.as_ptr());
        Ok(label)
    }
}

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    use super::*;
    use crate::appearance::tests::screen_brightness;
    use xila::graphics::Color;
    use xila::graphics::appearance::Appearance;

    pub async fn exercise_ui(
        context: &'static xila::executable::ExecutableContext,
        graphics: &'static xila::graphics::Manager,
    ) {
        let mut guard = graphics.lock().await;
        unsafe {
            let display = lvgl::lv_display_get_default();
            let saved = *lvgl::lv_theme_shadcn_get_palette(display);
            assert!(!saved.dark);
            let expected = Appearance {
                is_dark: false,
                primary: 2,
                secondary: 5,
                ..Appearance::default()
            }
            .colors();
            assert_eq!(Color::from(saved.primary), expected.0);
            assert_eq!(Color::from(saved.secondary), expected.1);

            // Keep assertion unwinding from dropping a window while this test
            // holds the LVGL lock. Successful runs clean up after unlocking.
            let mut window = core::mem::ManuallyDrop::new(
                guard
                    .with_lvgl(|lvgl| graphics.create_window(lvgl))
                    .unwrap()
                    .unwrap(),
            );
            let mut other = core::mem::ManuallyDrop::new(
                guard
                    .with_lvgl(|lvgl| graphics.create_window(lvgl))
                    .unwrap()
                    .unwrap(),
            );
            let button = lvgl::lv_button_create(other.as_object());
            let tabview = lvgl::lv_tabview_create(window.as_object());
            let mut tab = AppearanceTab::new(context).await;
            let container = tab.create_ui(tabview).await.unwrap();
            assert!(!container.is_null());
            assert_eq!(lvgl::lv_tabview_get_tab_count(tabview), 1);

            while guard
                .with_lvgl(|lvgl| window.pop_event(lvgl))
                .unwrap()
                .is_some()
            {}
            lvgl::lv_obj_send_event(
                tab.dark_mode,
                lvgl::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
                core::ptr::null_mut(),
            );
            let bubbled = guard
                .with_lvgl(|lvgl| window.pop_event(lvgl))
                .unwrap()
                .unwrap();
            assert_eq!(bubbled.code, EventKind::ValueChanged);
            assert!(bubbled.is_target(tab.dark_mode));

            assert_eq!(lvgl::lv_dropdown_get_selected(tab.primary), 2);
            assert_eq!(lvgl::lv_dropdown_get_selected(tab.secondary), 5);
            assert!(!lvgl::lv_obj_has_state(
                tab.dark_mode,
                lvgl::LV_STATE_CHECKED
            ));
            assert!(
                !tab.handle_event(
                    &Event::new(EventKind::Clicked, tab.dark_mode, None),
                    &mut guard
                )
                .await
            );

            lvgl::lv_obj_add_state(tab.dark_mode, lvgl::LV_STATE_CHECKED);
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::ValueChanged, tab.dark_mode, None),
                    &mut guard
                )
                .await
            );
            let palette = *lvgl::lv_theme_shadcn_get_palette(display);
            assert!(palette.dark);
            assert_eq!(Color::from(palette.primary), expected.0);
            assert_eq!(Color::from(palette.secondary), expected.1);

            for (control, selected) in [(tab.primary, 1), (tab.secondary, 3)] {
                lvgl::lv_dropdown_set_selected(control, selected);
                assert!(
                    tab.handle_event(
                        &Event::new(EventKind::ValueChanged, control, None),
                        &mut guard
                    )
                    .await
                );
            }
            let palette = *lvgl::lv_theme_shadcn_get_palette(display);
            let expected = Appearance {
                is_dark: true,
                primary: 1,
                secondary: 3,
                ..Appearance::default()
            };
            let colors = expected.colors();
            assert_eq!(Color::from(palette.primary), colors.0);
            assert_eq!(Color::from(palette.secondary), colors.1);
            assert_eq!(Color::from(palette.ring), colors.0);
            assert_eq!(
                Color::from(lvgl::lv_obj_get_style_bg_color(button, 0)),
                colors.0
            );
            assert_eq!(
                Color::from(lvgl::lv_obj_get_style_text_color(button, 0)),
                Color::WHITE
            );
            let task = context.task_manager.get_current_task_identifier().await;
            assert_eq!(
                Appearance::load(context.virtual_file_system, task)
                    .await
                    .unwrap(),
                expected
            );

            // The slider applies brightness while it is dragged, and saves
            // it once released.
            let load = || async {
                Appearance::load(context.virtual_file_system, task)
                    .await
                    .unwrap()
            };
            assert!(!tab.brightness.is_null());
            assert_eq!(lvgl::lv_slider_get_value(tab.brightness), 100);
            assert_eq!(screen_brightness(), xila::graphics::BRIGHTNESS_FULL);
            lvgl::lv_obj_add_state(tab.brightness, lvgl::LV_STATE_PRESSED);
            lvgl::lv_slider_set_value(tab.brightness, 40, false);
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::ValueChanged, tab.brightness, None),
                    &mut guard
                )
                .await
            );
            assert_eq!(screen_brightness(), percent_to_brightness(40));
            assert_eq!(load().await.brightness, xila::graphics::BRIGHTNESS_FULL);
            lvgl::lv_obj_remove_state(tab.brightness, lvgl::LV_STATE_PRESSED);
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::Released, tab.brightness, None),
                    &mut guard
                )
                .await
            );
            assert_eq!(load().await.brightness, percent_to_brightness(40));
            // A press lost to a scroll saves like a release.
            lvgl::lv_obj_add_state(tab.brightness, lvgl::LV_STATE_PRESSED);
            lvgl::lv_slider_set_value(tab.brightness, 60, false);
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::ValueChanged, tab.brightness, None),
                    &mut guard
                )
                .await
            );
            assert_eq!(load().await.brightness, percent_to_brightness(40));
            lvgl::lv_obj_remove_state(tab.brightness, lvgl::LV_STATE_PRESSED);
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::PressLost, tab.brightness, None),
                    &mut guard
                )
                .await
            );
            assert_eq!(load().await.brightness, percent_to_brightness(60));
            // Keypad steps save at once; the slider stops at the minimum.
            lvgl::lv_slider_set_value(tab.brightness, 0, false);
            assert_eq!(
                lvgl::lv_slider_get_value(tab.brightness),
                i32::from(MINIMUM_BRIGHTNESS_PERCENT)
            );
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::ValueChanged, tab.brightness, None),
                    &mut guard
                )
                .await
            );
            let minimum = percent_to_brightness(MINIMUM_BRIGHTNESS_PERCENT);
            assert_eq!(screen_brightness(), minimum);
            assert_eq!(load().await.brightness, minimum);
            // Other controls keep the saved brightness.
            lvgl::lv_dropdown_set_selected(tab.primary, 1);
            assert!(
                tab.handle_event(
                    &Event::new(EventKind::ValueChanged, tab.primary, None),
                    &mut guard
                )
                .await
            );
            assert_eq!(
                load().await,
                Appearance {
                    brightness: minimum,
                    ..expected
                }
            );

            let mut reopened = AppearanceTab::new(context).await;
            reopened.create_ui(tabview).await.unwrap();
            assert_eq!(
                lvgl::lv_slider_get_value(reopened.brightness),
                i32::from(MINIMUM_BRIGHTNESS_PERCENT)
            );
            assert_eq!(lvgl::lv_dropdown_get_selected(reopened.primary), 1);
            assert_eq!(lvgl::lv_dropdown_get_selected(reopened.secondary), 3);
            assert!(lvgl::lv_obj_has_state(
                reopened.dark_mode,
                lvgl::LV_STATE_CHECKED
            ));
            // OwnedWindow::drop acquires the graphics lock itself.
            drop(guard);
            core::mem::ManuallyDrop::drop(&mut other);
            core::mem::ManuallyDrop::drop(&mut window);
        }
    }
}
