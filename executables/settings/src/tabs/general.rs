use crate::error::Result;
use xila::graphics::{Event, lvgl};
use xila::internationalization::t;

use super::time_zone::TimeZoneSection;

pub struct GeneralTab {
    tab_view: *mut lvgl::lv_obj_t,
    tab_container: *mut lvgl::lv_obj_t,
    /// Boxed: the section is the largest part of the Settings tabs.
    time_zone: alloc::boxed::Box<TimeZoneSection>,
}

impl GeneralTab {
    /// The position of the tab in the tab view.
    const INDEX: u32 = 0;

    pub fn new(context: &'static xila::executable::ExecutableContext) -> Self {
        Self {
            tab_view: core::ptr::null_mut(),
            tab_container: core::ptr::null_mut(),
            time_zone: alloc::boxed::Box::new(TimeZoneSection::new(context)),
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

        self.tab_view = parent_tabview;
        self.tab_container = tab_container;

        // The language is fixed at build time; show it until it can be changed.
        unsafe {
            super::setup_page(tab_container);
            let row = super::setting_row(tab_container, t!(c"general.fields.language"))?;
            let locale = alloc::ffi::CString::new(xila::internationalization::locale!())?;
            let value = super::check(lvgl::lv_label_create(row))?;
            lvgl::lv_label_set_text(value, locale.as_ptr());
            lvgl::lv_obj_set_style_text_opa(value, super::MUTED, 0);
            self.time_zone.create_ui(tab_container).await?;
        }

        Ok(tab_container)
    }

    /// Whether the tab is shown, so that its clock preview needs updates.
    pub fn is_active(&self) -> bool {
        !self.tab_view.is_null()
            // SAFETY: the tab view is live and the caller holds the graphics lock.
            && unsafe { lvgl::lv_tabview_get_tab_active(self.tab_view) } == Self::INDEX
    }

    /// Update the local time preview. The caller holds the graphics lock.
    pub fn refresh(&mut self) {
        self.time_zone.refresh();
    }

    pub async fn handle_event(&mut self, event: &Event) -> bool {
        self.time_zone.handle_event(event).await
    }
}
