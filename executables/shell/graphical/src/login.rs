use alloc::{ffi::CString, string::ToString};
pub(crate) use core::ffi::CStr;
use xila::authentication;
use xila::graphics::{self, Color, EventKind, Logo, OwnedWindow, fonts, lvgl, theme};
use xila::internationalization::t;
use xila::users::UserIdentifier;

use crate::error::{Error, Result};

pub struct Login {
    context: &'static xila::executable::ExecutableContext,
    graphics_manager: &'static graphics::Manager,
    window: OwnedWindow,
    user_name_text_area: *mut lvgl::lv_obj_t,
    password_text_area: *mut lvgl::lv_obj_t,
    button: *mut lvgl::lv_obj_t,
    error_label: *mut lvgl::lv_obj_t,
    user: Option<UserIdentifier>,
}

impl Login {
    pub async fn new(context: &'static xila::executable::ExecutableContext) -> Result<Self> {
        let graphics_manager = graphics::ffi_manager();
        // - Lock the graphics
        graphics::lock!(graphics_manager, {
            // - Create a window
            let mut window = graphics_manager.create_window().await?;

            // One centered card: logo, the two fields, the error line and the
            // primary action. Small displays drop the logo to keep the form on
            // screen.
            let card = unsafe {
                let root = window.as_object_mutable();
                lvgl::lv_obj_update_layout(root);
                let compact =
                    lvgl::lv_obj_get_width(root) < 560 || lvgl::lv_obj_get_height(root) < 360;
                lvgl::lv_obj_set_flex_flow(root, lvgl::LV_FLEX_COLUMN);
                lvgl::lv_obj_set_flex_align(
                    root,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                );

                let card = lvgl::lv_obj_create(root);
                if card.is_null() {
                    return Err(Error::FailedToCreateObject);
                }
                theme::set_surface(card, theme::Surface::Card);
                lvgl::lv_obj_set_size(
                    card,
                    if compact { lvgl::lv_pct(90) } else { 320 },
                    lvgl::LV_SIZE_CONTENT,
                );
                lvgl::lv_obj_set_flex_flow(card, lvgl::LV_FLEX_COLUMN);
                lvgl::lv_obj_set_flex_align(
                    card,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                );
                let padding = if compact { 10 } else { 24 };
                lvgl::lv_obj_set_style_pad_all(card, padding, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_pad_row(
                    card,
                    if compact { 8 } else { 12 },
                    lvgl::LV_STATE_DEFAULT,
                );
                lvgl::lv_obj_remove_flag(card, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

                if !compact {
                    let logo = Logo::new(card, 2, Color::WHITE)?;
                    logo.follow_palette_color(theme::PaletteColor::Foreground);
                    lvgl::lv_obj_set_style_margin_bottom(
                        logo.get_inner_object(),
                        8,
                        lvgl::LV_STATE_DEFAULT,
                    );
                    // The card owns the widget tree from here on.
                    core::mem::forget(logo);
                }

                card
            };

            let user_name_text_area = unsafe {
                // - Create a text area for the user name
                let user_name_text_area = lvgl::lv_textarea_create(card);

                lvgl::lv_textarea_set_placeholder_text(
                    user_name_text_area,
                    t!(c"login.fields.username").as_ptr(),
                );
                lvgl::lv_textarea_set_one_line(user_name_text_area, true);
                lvgl::lv_obj_set_width(user_name_text_area, lvgl::lv_pct(100));

                user_name_text_area
            };

            let password_text_area = unsafe {
                // - Create a text area for the password
                let password_text_area = lvgl::lv_textarea_create(card);

                lvgl::lv_textarea_set_placeholder_text(
                    password_text_area,
                    t!(c"login.fields.password").as_ptr(),
                );
                lvgl::lv_textarea_set_one_line(password_text_area, true);
                lvgl::lv_textarea_set_password_mode(password_text_area, true);
                lvgl::lv_obj_set_width(password_text_area, lvgl::lv_pct(100));

                password_text_area
            };

            let button = unsafe {
                let button = lvgl::lv_button_create(card);
                lvgl::lv_obj_set_width(button, lvgl::lv_pct(100));

                let label = lvgl::lv_label_create(button);

                lvgl::lv_label_set_text(label, t!(c"login.actions.submit").as_ptr());
                lvgl::lv_obj_center(label);

                button
            };

            // Hidden until there is something to report, so the card does not
            // keep an empty line.
            let error_label = unsafe {
                let error_label = lvgl::lv_label_create(card);

                lvgl::lv_label_set_text(error_label, c"".as_ptr());
                lvgl::lv_obj_set_width(error_label, lvgl::lv_pct(100));
                lvgl::lv_label_set_long_mode(
                    error_label,
                    lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
                );
                lvgl::lv_obj_set_style_text_font(
                    error_label,
                    fonts::get_font_small(),
                    lvgl::LV_STATE_DEFAULT,
                );
                lvgl::lv_theme_shadcn_set_error(error_label, true);
                lvgl::lv_obj_add_flag(error_label, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);

                error_label
            };

            Ok(Login {
                context,
                graphics_manager,
                window,
                user_name_text_area,
                password_text_area,
                button,
                error_label,
                user: None,
            })
        })
    }

    pub fn print_error(&mut self, error: Error) {
        // The message needs its own terminator before LVGL copies it.
        let error = CString::new(error.to_string()).unwrap_or_default();

        unsafe {
            lvgl::lv_label_set_text(self.error_label, error.as_ptr());
            lvgl::lv_obj_remove_flag(self.error_label, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        }
    }

    pub fn clear_error(&mut self) {
        unsafe {
            lvgl::lv_label_set_text(self.error_label, c"".as_ptr());
            lvgl::lv_obj_add_flag(self.error_label, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
        }
    }

    pub async fn authenticate(&mut self) -> Result<()> {
        let (user_name, password) = unsafe {
            let user_name = lvgl::lv_textarea_get_text(self.user_name_text_area);
            let user_name = CStr::from_ptr(user_name);

            let user_name = user_name.to_str().map_err(Error::InvalidUtf8)?;

            let password = lvgl::lv_textarea_get_text(self.password_text_area);
            let password = CStr::from_ptr(password);
            let password = password.to_str().map_err(Error::InvalidUtf8)?;

            (user_name, password)
        };

        // - Check the user name and the password
        let task_manager = &self.context.task_manager;
        let task = task_manager.get_current_task_identifier().await;
        let authentication_context = authentication::Context {
            virtual_file_system: self.context.virtual_file_system,
            task_manager,
            users_manager: self.context.users_manager,
            task,
        };
        let user_identifier =
            authentication::authenticate_user(&authentication_context, user_name, password)
                .await
                .map_err(Error::AuthenticationFailed)?;

        let home = self
            .context
            .virtual_file_system
            .ensure_user_home(task, user_identifier)
            .await
            .map_err(Error::FailedToOpenDirectory)?;
        task_manager
            .set_environment_variable(task, "HOME", home.as_str())
            .await
            .map_err(Error::FailedToSetEnvironmentVariable)?;

        // - Set the user
        task_manager
            .set_user(task, user_identifier)
            .await
            .map_err(Error::FailedToSetTaskUser)?;

        self.user = Some(user_identifier);

        Ok(())
    }

    pub async fn event_handler(&mut self) {
        graphics::lock!(self.graphics_manager, {
            {
                while let Some(event) = self.window.pop_event() {
                    // If we are typing the user name or the password
                    if event.code == EventKind::ValueChanged
                        && (event.target == self.user_name_text_area
                            || event.target == self.password_text_area)
                    {
                        self.clear_error();
                    }
                    // If the "Login" button is clicked
                    else if event.code == EventKind::Clicked && event.target == self.button {
                        let result = self.authenticate().await;

                        if let Err(error) = result {
                            self.print_error(error);
                        }
                    }
                }
            }
        })
    }

    pub fn get_logged_user(&self) -> Option<UserIdentifier> {
        self.user
    }
}
