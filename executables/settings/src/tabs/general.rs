use crate::error::Result;
use xila::graphics::{Event, lvgl};
use xila::internationalization::t;

pub struct GeneralTab {
    tab_container: *mut lvgl::lv_obj_t,
}

impl GeneralTab {
    pub fn new() -> Self {
        Self {
            tab_container: core::ptr::null_mut(),
        }
    }

    pub async fn create_ui(
        &mut self,
        parent_tabview: *mut lvgl::lv_obj_t,
    ) -> Result<*mut lvgl::lv_obj_t> {
        let tab_container =
            unsafe { lvgl::lv_tabview_add_tab(parent_tabview, t!(c"tabs.general").as_ptr()) };

        if tab_container.is_null() {
            return Err(crate::error::Error::FailedToCreateUiElement);
        }

        self.tab_container = tab_container;

        // The language is fixed at build time; show it until it can be changed.
        unsafe {
            super::setup_page(tab_container);
            let row = super::setting_row(tab_container, t!(c"general.fields.language"))?;
            let locale = alloc::ffi::CString::new(xila::internationalization::locale!())?;
            let value = super::check(lvgl::lv_label_create(row))?;
            lvgl::lv_label_set_text(value, locale.as_ptr());
            lvgl::lv_obj_set_style_text_opa(value, super::MUTED, 0);
            super::caption(tab_container, t!(c"general.status.coming_soon"))?;
        }

        Ok(tab_container)
    }

    pub async fn handle_event(&mut self, _event: &Event) -> bool {
        // For now, the general tab doesn't handle any specific events
        false
    }
}
