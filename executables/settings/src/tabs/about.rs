use crate::error::Result;
use alloc::{ffi::CString, format, string::ToString, vec::Vec};
use core::{ffi::CStr, ptr::null_mut, str};
use xila::{
    about,
    graphics::{
        Event,
        lvgl::{self},
    },
    internationalization::{self, t},
    memory,
    shared::{BYTES_SUFFIX, Unit},
    virtual_file_system::File,
};

pub struct AboutTab {
    context: &'static xila::executable::ExecutableContext,
    container: *mut lvgl::lv_obj_t,
    list: *mut lvgl::lv_obj_t,
}

impl AboutTab {
    pub fn new(context: &'static xila::executable::ExecutableContext) -> Self {
        Self {
            context,
            container: null_mut() as *mut _,
            list: null_mut() as *mut _,
        }
    }

    pub async fn create_ui(
        &mut self,
        parent_tabview: *mut lvgl::lv_obj_t,
    ) -> Result<*mut lvgl::lv_obj_t> {
        self.container =
            unsafe { lvgl::lv_tabview_add_tab(parent_tabview, t!(c"tabs.about").as_ptr()) };

        if self.container.is_null() {
            return Err(crate::error::Error::FailedToCreateUiElement);
        }

        // One card of name and value rows.
        unsafe {
            super::setup_page(self.container);
            self.list = super::card(self.container)?;
            lvgl::lv_obj_set_style_pad_ver(self.list, 4, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_row(self.list, 0, lvgl::LV_STATE_DEFAULT);
        }

        // Populate items - convert CStr to String
        self.create_list_item(t!(c"about.fields.operating_system"), c"Xila")?;

        let description = CString::new(about::get_description())
            .map_err(|_| crate::error::Error::FailedToCreateUiElement)?;
        self.create_list_item(t!(c"about.fields.description"), &description)?;

        let authors = CString::new(about::get_authors())
            .map_err(|_| crate::error::Error::FailedToCreateUiElement)?;
        self.create_list_item(t!(c"about.fields.developer"), &authors)?;

        let license = CString::new(about::get_license())
            .map_err(|_| crate::error::Error::FailedToCreateUiElement)?;
        self.create_list_item(t!(c"about.fields.license"), &license)?;

        let version = CString::new(about::get_version_string())
            .map_err(|_| crate::error::Error::FailedToCreateUiElement)?;
        self.create_list_item(t!(c"about.fields.version"), &version)?;

        let locale = CString::new(internationalization::locale!())?;
        self.create_list_item(t!(c"general.fields.locale"), &locale)?;

        let memory = memory::global_manager().get_total_size();
        let memory = Unit::new(memory as f32, BYTES_SUFFIX.symbol);
        let memory = CString::new(format!("{}", memory))
            .map_err(|_| crate::error::Error::FailedToCreateUiElement)?;
        self.create_list_item(t!(c"about.fields.memory"), &memory)?;

        let cpu_summary = CString::new(self.get_cpu_summary().await)
            .map_err(|_| crate::error::Error::FailedToCreateUiElement)?;
        self.create_list_item(t!(c"about.fields.cpu"), &cpu_summary)?;

        Ok(self.container)
    }

    async fn get_cpu_summary(&self) -> alloc::string::String {
        let virtual_file_system = &self.context.virtual_file_system;
        let task = self
            .context
            .task_manager
            .get_current_task_identifier()
            .await;

        let mut buffer = Vec::new();

        if File::read_from_path(
            virtual_file_system,
            task,
            "/devices/compute/informations",
            &mut buffer,
        )
        .await
        .is_err()
        {
            return "unknown (unknown cores, unknown)".to_string();
        }

        let content = str::from_utf8(&buffer).unwrap_or_default();

        let model_name = Self::get_cpu_info_value(content, "model name").unwrap_or("unknown");
        let cpu_cores = Self::get_cpu_info_value(content, "cpu cores").unwrap_or("unknown");
        let architecture = Self::get_cpu_info_value(content, "architecture").unwrap_or("unknown");

        format!("{} ({} cores, {})", model_name, cpu_cores, architecture)
    }

    fn get_cpu_info_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
        for line in content.lines() {
            let Some((line_key, line_value)) = line.split_once(':') else {
                continue;
            };

            if line_key.trim() == key {
                return Some(line_value.trim());
            }
        }

        None
    }

    fn create_list_item(&mut self, name: &CStr, value: &CStr) -> Result<()> {
        unsafe {
            // Name on the left, value on the right, wrapping when it is long.
            let row = super::check(lvgl::lv_obj_create(self.list))?;
            lvgl::lv_obj_set_size(row, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
            lvgl::lv_obj_set_style_bg_opa(row, lvgl::LV_OPA_TRANSP, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(row, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_hor(row, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_ver(row, 10, lvgl::LV_STATE_DEFAULT);
            // A separator under every row but the last one added so far.
            lvgl::lv_obj_set_style_border_width(row, 0, lvgl::LV_STATE_DEFAULT);
            let index = lvgl::lv_obj_get_index(row);
            if index > 0 {
                let previous = lvgl::lv_obj_get_child(self.list, index - 1);
                lvgl::lv_obj_set_style_border_width(previous, 1, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_border_side(
                    previous,
                    lvgl::lv_border_side_t_LV_BORDER_SIDE_BOTTOM,
                    lvgl::LV_STATE_DEFAULT,
                );
            }
            lvgl::lv_obj_set_flex_flow(row, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lvgl::lv_obj_remove_flag(row, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

            let name = super::caption(row, name)?;
            lvgl::lv_obj_set_width(
                name,
                lvgl::lv_pct(if super::is_compact() { 38 } else { 28 }),
            );
            let value_label = super::check(lvgl::lv_label_create(row))?;
            lvgl::lv_label_set_text(value_label, value.as_ptr());
            lvgl::lv_obj_set_flex_grow(value_label, 1);
            lvgl::lv_label_set_long_mode(
                value_label,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_WRAP,
            );
        }

        Ok(())
    }

    pub async fn handle_event(&mut self, _event: &Event) -> bool {
        // For now, the about tab doesn't handle any specific events
        false
    }
}
