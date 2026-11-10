use crate::error::{Error, Result};
use core::ffi::CStr;
use xila::graphics::{Event, fonts, lvgl, theme};

/// Opacity of secondary text; dimming follows the palette.
pub(crate) const MUTED: u8 = 150;

/// Whether the display is too small for the regular spacing.
pub(crate) unsafe fn is_compact() -> bool {
    unsafe { lvgl::lv_display_get_horizontal_resolution(core::ptr::null_mut()) < 560 }
}

/// Lays a tab out as a padded, scrolling column of rows and cards.
///
/// # Safety
/// `tab` must be live and the caller must hold the LVGL lock, as for every
/// helper in this module.
pub(crate) unsafe fn setup_page(tab: *mut lvgl::lv_obj_t) {
    unsafe {
        let (padding, gap) = if is_compact() { (8, 8) } else { (16, 12) };
        lvgl::lv_obj_set_flex_flow(tab, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
        lvgl::lv_obj_set_style_pad_all(tab, padding, 0);
        lvgl::lv_obj_set_style_pad_row(tab, gap, 0);
    }
}

pub(crate) fn check(object: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t> {
    if object.is_null() {
        Err(Error::FailedToCreateUiElement)
    } else {
        Ok(object)
    }
}

/// A card as wide as the page and as tall as its content.
pub(crate) unsafe fn card(parent: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let card = check(lvgl::lv_obj_create(parent))?;
        theme::set_surface(card, theme::Surface::Card);
        lvgl::lv_obj_set_size(card, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
        lvgl::lv_obj_set_flex_flow(card, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
        lvgl::lv_obj_remove_flag(card, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        Ok(card)
    }
}

/// A card holding one setting: its name on the left, and room on the right
/// for the control the caller adds.
pub(crate) unsafe fn setting_row(
    parent: *mut lvgl::lv_obj_t,
    name: &CStr,
) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let row = card(parent)?;
        lvgl::lv_obj_set_flex_flow(row, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
        lvgl::lv_obj_set_flex_align(
            row,
            lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
            lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
        );
        let label = check(lvgl::lv_label_create(row))?;
        lvgl::lv_label_set_text(label, name.as_ptr());
        lvgl::lv_obj_set_flex_grow(label, 1);
        Ok(row)
    }
}

/// Small, dimmed text for field names and notes.
pub(crate) unsafe fn caption(
    parent: *mut lvgl::lv_obj_t,
    text: &CStr,
) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let label = check(lvgl::lv_label_create(parent))?;
        lvgl::lv_label_set_text(label, text.as_ptr());
        lvgl::lv_obj_set_width(label, lvgl::lv_pct(100));
        lvgl::lv_label_set_long_mode(label, lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP);
        lvgl::lv_obj_set_style_text_font(label, fonts::get_font_small(), 0);
        lvgl::lv_obj_set_style_text_opa(label, MUTED, 0);
        Ok(label)
    }
}

/// Enum to hold all tab types (avoids dyn compatibility issues with async traits)
pub enum Tab {
    General(GeneralTab),
    Appearance(AppearanceTab),
    Password(PasswordTab),
    About(AboutTab),
    Network(NetworkTab),
}

impl Tab {
    pub async fn create_ui(&mut self, parent: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t> {
        match self {
            Tab::General(tab) => tab.create_ui(parent).await,
            Tab::Appearance(tab) => tab.create_ui(parent).await,
            Tab::Password(tab) => tab.create_ui(parent).await,
            Tab::About(tab) => tab.create_ui(parent).await,
            Tab::Network(tab) => tab.create_ui(parent).await,
        }
    }

    pub async fn handle_event(&mut self, event: &Event) -> bool {
        match self {
            Tab::General(tab) => tab.handle_event(event).await,
            Tab::Appearance(tab) => tab.handle_event(event).await,
            Tab::Password(tab) => tab.handle_event(event).await,
            Tab::About(tab) => tab.handle_event(event).await,
            Tab::Network(tab) => tab.handle_event(event).await,
        }
    }
}

// Re-export tab modules
pub mod about;
pub mod appearance;
pub mod general;
pub mod network;
pub mod password;

pub use about::AboutTab;
pub use appearance::AppearanceTab;
pub use general::GeneralTab;
pub use network::NetworkTab;
pub use password::PasswordTab;
