use alloc::ffi::CString;
use xila::graphics::{
    Color, Point,
    fonts::{get_font_extra_large, get_font_large},
    lvgl,
};

use crate::error::{Error, Result};

pub unsafe fn create_icon(
    parent: *mut lvgl::lv_obj_t,
    icon_color: Color,
    icon_string: &str,
    size: Point,
) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let icon = lvgl::lv_button_create(parent);

        if icon.is_null() {
            return Err(Error::FailedToCreateObject);
        }

        lvgl::lv_obj_set_size(icon, size.get_x().into(), size.get_y().into());

        lvgl::lv_obj_set_style_bg_color(icon, icon_color.into_lvgl_color(), lvgl::LV_STATE_DEFAULT);
        lvgl::lv_obj_set_style_text_color(
            icon,
            Color::WHITE.into_lvgl_color(),
            lvgl::LV_STATE_DEFAULT,
        );

        let label = lvgl::lv_label_create(icon);

        if label.is_null() {
            return Err(Error::FailedToCreateObject);
        }

        if size.get_x() >= 48 {
            lvgl::lv_obj_set_style_text_font(label, get_font_extra_large(), lvgl::LV_STATE_DEFAULT);
        } else {
            lvgl::lv_obj_set_style_text_font(label, get_font_large(), lvgl::LV_STATE_DEFAULT);
        }

        let icon_string = CString::new(icon_string).map_err(Error::NullCharacterInString)?;

        lvgl::lv_label_set_text(label, icon_string.as_ptr());
        lvgl::lv_obj_set_align(label, lvgl::lv_align_t_LV_ALIGN_CENTER);

        Ok(icon)
    }
}

/// Dots per side of the "show applications" grid.
const GRID_DOTS: i32 = 3;

/// Geometry of the "show applications" grid inside a square of `size` pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GridLayout {
    dot: i32,
    gap: i32,
    radius: i32,
    offset: i32,
}

impl GridLayout {
    /// Nine rounded squares covering three quarters of the button, with gaps
    /// half a dot wide, like the GNOME "show applications" symbol.
    const fn new(size: i32) -> Self {
        let dot = if size * 3 / 16 > 1 { size * 3 / 16 } else { 1 };
        let gap = if dot / 2 > 1 { dot / 2 } else { 1 };
        let extent = GRID_DOTS * dot + (GRID_DOTS - 1) * gap;

        Self {
            dot,
            gap,
            radius: (dot + 2) / 4,
            offset: (size - extent) / 2,
        }
    }
}

unsafe extern "C" fn draw_application_grid(event: *mut lvgl::lv_event_t) {
    unsafe {
        let object = lvgl::lv_event_get_current_target(event) as *mut lvgl::lv_obj_t;
        let layer = lvgl::lv_event_get_layer(event);
        let mut bounds = lvgl::lv_area_t::default();
        lvgl::lv_obj_get_coords(object, &mut bounds);

        let width = bounds.x2 - bounds.x1 + 1;
        let height = bounds.y2 - bounds.y1 + 1;
        let layout = GridLayout::new(width.min(height));
        let left = bounds.x1 + (width - width.min(height)) / 2 + layout.offset;
        let top = bounds.y1 + (height - width.min(height)) / 2 + layout.offset;

        // The dots use the button's text color, so the theme variant decides
        // their color in every state and palette.
        let mut descriptor = lvgl::lv_draw_rect_dsc_t::default();
        lvgl::lv_draw_rect_dsc_init(&mut descriptor);
        descriptor.bg_color = lvgl::lv_obj_get_style_text_color(object, lvgl::LV_PART_MAIN);
        descriptor.bg_opa = lvgl::lv_obj_get_style_text_opa(object, lvgl::LV_PART_MAIN);
        descriptor.radius = layout.radius;

        let step = layout.dot + layout.gap;
        for row in 0..GRID_DOTS {
            for column in 0..GRID_DOTS {
                let x1 = left + column * step;
                let y1 = top + row * step;
                let area = lvgl::lv_area_t {
                    x1,
                    y1,
                    x2: x1 + layout.dot - 1,
                    y2: y1 + layout.dot - 1,
                };
                lvgl::lv_draw_rect(layer, &descriptor, &area);
            }
        }
    }
}

/// Create the dock button that opens the application drawer: a ghost button
/// showing a 3 × 3 grid of rounded squares.
pub unsafe fn create_show_applications_button(
    parent: *mut lvgl::lv_obj_t,
    size: Point,
) -> Result<*mut lvgl::lv_obj_t> {
    unsafe {
        let button = lvgl::lv_button_create(parent);

        if button.is_null() {
            return Err(Error::FailedToCreateObject);
        }

        lvgl::lv_obj_set_size(button, size.get_x().into(), size.get_y().into());
        // The grid is drawn directly; drop the control's flex layout, padding
        // and minimum touch-control height so the button keeps its exact size.
        lvgl::lv_obj_set_style_layout(
            button,
            lvgl::lv_layout_t_LV_LAYOUT_NONE as u16,
            lvgl::LV_STATE_DEFAULT,
        );
        lvgl::lv_obj_set_style_min_height(button, 0, lvgl::LV_STATE_DEFAULT);
        lvgl::lv_obj_set_style_pad_all(button, 0, lvgl::LV_STATE_DEFAULT);
        lvgl::lv_theme_shadcn_set_variant(
            button,
            lvgl::lv_theme_shadcn_variant_t_LV_THEME_SHADCN_VARIANT_GHOST,
        );
        lvgl::lv_obj_add_event_cb(
            button,
            Some(draw_application_grid),
            lvgl::lv_event_code_t_LV_EVENT_DRAW_MAIN_END,
            core::ptr::null_mut(),
        );

        Ok(button)
    }
}

#[cfg(test)]
mod tests {
    use super::GridLayout;

    #[test]
    fn application_grid_is_centered_with_half_dot_gaps() {
        assert_eq!(
            GridLayout::new(32),
            GridLayout {
                dot: 6,
                gap: 3,
                radius: 2,
                offset: 4,
            }
        );
        for size in [16, 24, 32, 40, 48, 64] {
            let layout = GridLayout::new(size);
            let extent = 3 * layout.dot + 2 * layout.gap;
            assert!(extent <= size);
            assert!(layout.offset >= 0);
            assert!((size - extent - 2 * layout.offset).abs() <= 1);
            assert!(layout.radius * 2 < layout.dot);
        }
    }
}
