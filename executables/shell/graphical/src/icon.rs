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

#[cfg(all(test, target_os = "linux"))]
static CALLBACK_REGISTRATION_COUNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
#[cfg(all(test, target_os = "linux"))]
static FAIL_CALLBACK_REGISTRATION_AT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

unsafe fn add_icon_callback(
    icon: *mut lvgl::lv_obj_t,
    callback: lvgl::lv_event_cb_t,
    code: lvgl::lv_event_code_t,
    data: *mut core::ffi::c_void,
) -> *mut lvgl::lv_event_dsc_t {
    #[cfg(all(test, target_os = "linux"))]
    {
        use core::sync::atomic::Ordering;
        let count = CALLBACK_REGISTRATION_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        if count == FAIL_CALLBACK_REGISTRATION_AT.load(Ordering::SeqCst) {
            return core::ptr::null_mut();
        }
    }
    unsafe { lvgl::lv_obj_add_event_cb(icon, callback, code, data) }
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
        // Install the owner first: once it succeeds, deleting the icon frees
        // the descriptor even if installing the drawing callback fails.
        if add_icon_callback(
            icon,
            Some(delete_icon),
            lvgl::lv_event_code_t_LV_EVENT_DELETE,
            data,
        )
        .is_null()
        {
            drop(Box::from_raw(data.cast::<IconSpec>()));
            lvgl::lv_obj_delete(icon);
            return Err(Error::FailedToCreateObject);
        }
        if add_icon_callback(
            icon,
            Some(draw_icon),
            lvgl::lv_event_code_t_LV_EVENT_DRAW_MAIN,
            data,
        )
        .is_null()
        {
            lvgl::lv_obj_delete(icon);
            return Err(Error::FailedToCreateObject);
        }
        Ok(icon)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod allocation_tests {
    use super::*;
    use core::sync::atomic::Ordering;
    use core::task::{Context, Poll};
    use xila::file_system::{
        ControlCommand, ControlCommandIdentifier, DirectBaseOperations, DirectCharacterDevice,
        MountOperations, Size,
    };
    use xila::graphics::{GET_RESOLUTION, SET_DRAWING_AREA, WAS_RESIZED};
    use xila::shared::AnyByLayout;
    extern crate abi_definitions;
    drivers_std::instantiate_global_allocator!();

    struct Screen;
    impl MountOperations for Screen {}
    impl DirectCharacterDevice for Screen {}
    impl DirectBaseOperations for Screen {
        fn read(
            &self,
            _: &mut Context<'_>,
            _: &mut [u8],
            _: Size,
        ) -> Poll<xila::file_system::Result<usize>> {
            Poll::Ready(Err(xila::file_system::Error::UnsupportedOperation))
        }
        fn write(
            &self,
            _: &mut Context<'_>,
            bytes: &[u8],
            _: Size,
        ) -> Poll<xila::file_system::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }
        fn control(
            &self,
            _: &mut Context<'_>,
            command: ControlCommandIdentifier,
            _: &AnyByLayout,
            output: &mut AnyByLayout,
        ) -> Poll<xila::file_system::Result<()>> {
            Poll::Ready(match command {
                GET_RESOLUTION::IDENTIFIER => {
                    GET_RESOLUTION::cast_output(output).map(|value| *value = Point::new(320, 240))
                }
                WAS_RESIZED::IDENTIFIER => {
                    WAS_RESIZED::cast_output(output).map(|value| *value = false)
                }
                SET_DRAWING_AREA::IDENTIFIER => Ok(()),
                _ => Err(xila::file_system::Error::UnsupportedOperation),
            })
        }
    }

    #[test]
    fn failed_callback_registration_removes_the_partial_icon() {
        let time = Box::leak(Box::new(
            time::Manager::new(&drivers_std::devices::TimeDevice).unwrap(),
        ));
        let manager = task::block_on(xila::graphics::initialize_with_input_devices(
            time,
            &Screen,
            &[],
            1024,
            false,
        ));
        let parent = manager.get_current_screen().unwrap();
        let before = unsafe { lvgl::lv_obj_get_child_count(parent) };
        for fail_at in [1, 2] {
            CALLBACK_REGISTRATION_COUNT.store(0, Ordering::SeqCst);
            FAIL_CALLBACK_REGISTRATION_AT.store(fail_at, Ordering::SeqCst);
            let icon = IconSpec::automatic("test", "T", xila::graphics::Color::BLACK);
            assert!(unsafe { create_icon(parent, icon, Point::new(40, 40)) }.is_err());
            assert_eq!(unsafe { lvgl::lv_obj_get_child_count(parent) }, before);
        }
        FAIL_CALLBACK_REGISTRATION_AT.store(0, Ordering::SeqCst);
        core::mem::forget(manager);
    }
}
