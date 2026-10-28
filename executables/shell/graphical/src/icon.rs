use alloc::{boxed::Box, ffi::CString};
use xila::graphics::{
    IconMotif, IconSpec, Point,
    fonts::{get_font_extra_large, get_font_large},
    lvgl,
};

use crate::error::{Error, Result};

/// Draw a bounded motif using stack descriptors; no image buffer or allocation
/// is created by this callback. LVGL owns any drawing tasks it schedules.
unsafe extern "C" fn draw_icon(event: *mut lvgl::lv_event_t) {
    unsafe {
        let icon = &*lvgl::lv_event_get_user_data(event).cast::<IconSpec>();
        let object = lvgl::lv_event_get_target(event).cast::<lvgl::lv_obj_t>();
        let mut bounds = lvgl::lv_area_t::default();
        lvgl::lv_obj_get_coords(object, &mut bounds);
        let width = bounds.x2 - bounds.x1 + 1;
        let height = bounds.y2 - bounds.y1 + 1;
        if width <= 0 || height <= 0 {
            return;
        }
        let layer = lvgl::lv_event_get_layer(event);
        if layer.is_null() {
            return;
        }
        let mut dsc = lvgl::lv_draw_rect_dsc_t::default();
        lvgl::lv_draw_rect_dsc_init(&mut dsc);
        dsc.bg_color = icon.accent.into_lvgl_color();
        dsc.bg_opa = 48;
        // A small deterministic shift provides variation while keeping every
        // motif within its clipped icon bounds.
        let shift = (icon.seed % 3) as i32;
        dsc.radius = 2;
        let rectangle = |x: i32, y: i32, w: i32, h: i32| {
            let area = lvgl::lv_area_t {
                x1: bounds.x1 + width * x / 100,
                y1: bounds.y1 + height * y / 100,
                x2: bounds.x1 + width * (x + w) / 100 - 1,
                y2: bounds.y1 + height * (y + h) / 100 - 1,
            };
            if area.x1 <= area.x2 && area.y1 <= area.y2 {
                lvgl::lv_draw_rect(layer, &dsc, &area);
            }
        };
        match icon.motif {
            IconMotif::Rings => {
                rectangle(8, 8, 84, 5);
                rectangle(8, 87, 84, 5);
                rectangle(8, 13, 5, 74);
                rectangle(87, 13, 5, 74);
            }
            IconMotif::Diagonal => {
                for i in 0..4 {
                    rectangle(5 + 20 * i, 8 + shift + 20 * i, 15, 6);
                }
            }
            IconMotif::Grid => {
                for y in [12, 42, 72] {
                    for x in [12, 42, 72] {
                        rectangle(x, y, 14, 14);
                    }
                }
            }
            IconMotif::Steps => {
                for i in 0..4 {
                    rectangle(6 + 22 * i, 72 - 16 * i, 16, 18);
                }
            }
            IconMotif::Orbit => {
                rectangle(5, 17, 90, 5);
                rectangle(5, 78, 90, 5);
                rectangle(10, 27, 6, 46);
                rectangle(84, 27, 6, 46);
            }
            IconMotif::Rays => {
                for i in 0..4 {
                    rectangle(8 + shift + 23 * i, 8, 7, 84);
                }
            }
        }
    }
}

unsafe extern "C" fn delete_icon(event: *mut lvgl::lv_event_t) {
    unsafe {
        drop(Box::from_raw(
            lvgl::lv_event_get_user_data(event).cast::<IconSpec>(),
        ));
    }
}

pub unsafe fn create_icon(
    parent: *mut lvgl::lv_obj_t,
    spec: IconSpec,
    size: Point,
) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let icon = lvgl::lv_button_create(parent);
        if icon.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        lvgl::lv_obj_set_size(icon, size.get_x().into(), size.get_y().into());
        lvgl::lv_obj_set_style_bg_color(
            icon,
            spec.background.into_lvgl_color(),
            lvgl::LV_STATE_DEFAULT,
        );
        lvgl::lv_obj_set_style_text_color(
            icon,
            spec.foreground.into_lvgl_color(),
            lvgl::LV_STATE_DEFAULT,
        );
        let label = lvgl::lv_label_create(icon);
        if label.is_null() {
            lvgl::lv_obj_delete(icon);
            return Err(Error::FailedToCreateObject);
        }
        let font = if size.get_x() >= 48 {
            get_font_extra_large()
        } else {
            get_font_large()
        };
        lvgl::lv_obj_set_style_text_font(label, font, lvgl::LV_STATE_DEFAULT);
        let text = CString::new(spec.text()).map_err(Error::NullCharacterInString)?;
        lvgl::lv_label_set_text(label, text.as_ptr());
        lvgl::lv_obj_set_align(label, lvgl::lv_align_t_LV_ALIGN_CENTER);
        let data = Box::into_raw(Box::new(spec)).cast();
        lvgl::lv_obj_add_event_cb(
            icon,
            Some(draw_icon),
            lvgl::lv_event_code_t_LV_EVENT_DRAW_MAIN,
            data,
        );
        lvgl::lv_obj_add_event_cb(
            icon,
            Some(delete_icon),
            lvgl::lv_event_code_t_LV_EVENT_DELETE,
            data,
        );
        Ok(icon)
    }
}
