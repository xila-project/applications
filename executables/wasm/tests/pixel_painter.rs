#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
#[path = "support/applications.rs"]
mod applications;
extern crate abi_definitions;
testing::instantiate_global_allocator!();
#[path = "support/localization.rs"]
mod localization;
#[path = "support/graphics.rs"]
mod support;

use core::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use xila::{
    graphics::{Manager, lvgl},
    virtual_file_system::File,
};

// Search descendants by class/text so layout containers can change freely.
unsafe fn find(
    root: *mut lvgl::lv_obj_t,
    predicate: &impl Fn(*mut lvgl::lv_obj_t) -> bool,
) -> *mut lvgl::lv_obj_t {
    unsafe {
        if predicate(root) {
            return root;
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find(lvgl::lv_obj_get_child(root, index as i32), predicate);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

unsafe fn label(root: *mut lvgl::lv_obj_t, text: &str) -> *mut lvgl::lv_obj_t {
    unsafe {
        find(root, &|object| {
            lvgl::lv_obj_has_class(object, &lvgl::lv_label_class)
                && core::ffi::CStr::from_ptr(lvgl::lv_label_get_text(object)).to_string_lossy()
                    == text
        })
    }
}

unsafe fn class(
    root: *mut lvgl::lv_obj_t,
    class: *const lvgl::lv_obj_class_t,
) -> *mut lvgl::lv_obj_t {
    unsafe { find(root, &|object| lvgl::lv_obj_has_class(object, class)) }
}

async fn status(manager: &Manager, expected: &str) {
    let expected = localization::text("pixel_painter", expected);
    for _ in 0..400 {
        if manager.get_window_count().await.unwrap() > 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let _lock = manager.synchronous_lock();
            if unsafe { !label(root, &expected).is_null() } {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Pixel Painter did not show {expected:?}");
}

async fn click(manager: &Manager, key: &str) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let text = localization::text("pixel_painter", key);
        let label = label(root, &text);
        assert!(!label.is_null(), "missing control {text}");
        let button = lvgl::lv_obj_get_parent(label);
        assert!(lvgl::lv_obj_has_class(button, &lvgl::lv_button_class));
        lvgl::lv_obj_update_layout(root);
        let mut bounds = lvgl::lv_area_t::default();
        let mut window = lvgl::lv_area_t::default();
        lvgl::lv_obj_get_coords(button, &mut bounds);
        lvgl::lv_obj_get_content_coords(root, &mut window);
        assert!(
            bounds.x1 >= window.x1
                && bounds.x2 <= window.x2
                && bounds.y1 >= window.y1
                && bounds.y2 <= window.y2,
            "control {text} must fit the compact window"
        );
        lvgl::lv_obj_send_event(
            button,
            lvgl::lv_event_code_t_LV_EVENT_CLICKED,
            core::ptr::null_mut(),
        );
    }
}

async fn action(manager: &Manager, button: u32) {
    click(
        manager,
        if button >= 2 {
            "controls.file"
        } else {
            "controls.paint"
        },
    )
    .await;
    xila::task::sleep(Duration::from_millis(30)).await;
    if button == 1 {
        click(manager, "controls.tools").await;
        xila::task::sleep(Duration::from_millis(30)).await;
    }
    click(
        manager,
        [
            "controls.undo",
            "controls.clear",
            "controls.save",
            "controls.open",
        ][button as usize],
    )
    .await;
}

#[xila::task::test(task_path=xila::task)]
async fn real_painter_guest_draws_saves_opens_and_preserves_work_on_errors() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let task = context.task_manager.get_current_task_identifier().await;
    let vfs = context.virtual_file_system;
    vfs.ensure_user_application_directories(
        task,
        xila::users::UserIdentifier::ROOT,
        "pixel_painter",
    )
    .await
    .unwrap();
    let binary = applications::build("xila-pixel-painter").unwrap();
    testing::load_file(vfs, binary, "/applications/executables/pixel_painter")
        .await
        .unwrap();
    xila::executable::mount_executables!(
        context,
        vfs,
        task,
        &[("/applications/executables/wasm", wasm::WasmExecutable)]
    )
    .await
    .unwrap();
    let handle = xila::executable::execute(
        context,
        "/applications/executables/wasm",
        vec!["/applications/executables/pixel_painter".into()],
        standard,
        None,
    )
    .await
    .unwrap();
    status(manager, "status.ready_white_erases").await;
    // Queue two distinct pointer positions before the guest drains either one.
    // Re-reading the current input position at dequeue time would lose the start.
    {
        let _lock = manager.synchronous_lock();
        let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
        unsafe {
            lvgl::lv_obj_update_layout(root);
            let canvas = find(root, &|object| {
                lvgl::lv_obj_check_type(object, &lvgl::lv_obj_class)
                    && lvgl::lv_obj_get_child_count(object) == 0
                    && lvgl::lv_obj_has_flag(object, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_CLICKABLE)
            });
            assert!(!canvas.is_null());
            let mut area = lvgl::lv_area_t::default();
            lvgl::lv_obj_get_content_coords(canvas, &mut area);
            assert!(
                area.y2 - area.y1 >= 100,
                "compact canvas needs usable height"
            );
            let input = lvgl::lv_indev_create();
            lvgl::lv_indev_set_type(input, lvgl::lv_indev_type_t_LV_INDEV_TYPE_POINTER);
            let x = (area.x1 + area.x2) / 2;
            let y = (area.y1 + area.y2) / 2;
            (*input).pointer.act_point = lvgl::lv_point_t { x: x - 5, y };
            lvgl::lv_obj_send_event(canvas, lvgl::lv_event_code_t_LV_EVENT_PRESSED, input.cast());
            (*input).pointer.act_point.x = x + 5;
            lvgl::lv_obj_send_event(
                canvas,
                lvgl::lv_event_code_t_LV_EVENT_PRESSING,
                input.cast(),
            );
            lvgl::lv_obj_send_event(
                canvas,
                lvgl::lv_event_code_t_LV_EVENT_RELEASED,
                input.cast(),
            );
            lvgl::lv_indev_delete(input);
        }
    }
    xila::task::sleep(Duration::from_millis(50)).await;
    action(manager, 2).await;
    status(manager, "status.saved").await;
    let path = "/users/0/applications/pixel_painter/data/drawing.xpnt";
    let mut saved = Vec::new();
    File::read_from_path(vfs, task, path, &mut saved)
        .await
        .unwrap();
    assert_eq!(saved.len(), 16_008);
    assert!(
        saved[8..].iter().filter(|&&pixel| pixel == 1).count() > 1,
        "pointer positions must produce a continuous stroke"
    );
    action(manager, 1).await; // Clear.
    xila::task::sleep(Duration::from_millis(30)).await;
    action(manager, 0).await; // Undo clear.
    xila::task::sleep(Duration::from_millis(30)).await;
    action(manager, 2).await;
    status(manager, "status.saved").await;
    let mut after_undo = Vec::new();
    File::read_from_path(vfs, task, path, &mut after_undo)
        .await
        .unwrap();
    assert_eq!(after_undo, saved);
    action(manager, 3).await;
    status(manager, "status.opened").await;
    File::write_to_path(vfs, task, path, b"broken")
        .await
        .unwrap();
    action(manager, 3).await;
    status(manager, "errors.invalid_file_drawing_preserved").await;
    action(manager, 2).await;
    status(manager, "status.saved").await;
    File::read_from_path(vfs, task, path, &mut after_undo)
        .await
        .unwrap();
    assert_eq!(after_undo, saved);
    action(manager, 1).await;
    xila::task::sleep(Duration::from_millis(30)).await;
    action(manager, 3).await;
    status(manager, "warnings.unsaved_open_again").await;
    action(manager, 3).await;
    status(manager, "status.opened").await;
    let root = manager.get_window_identifier(0).await.unwrap();
    {
        let _lock = manager.synchronous_lock();
        let long_path = std::ffi::CString::new(format!("/{}", "é".repeat(130))).unwrap();
        unsafe {
            let input = class(root as *mut lvgl::lv_obj_t, &lvgl::lv_textarea_class);
            lvgl::lv_textarea_set_text(input, long_path.as_ptr());
        }
    }
    action(manager, 2).await;
    status(manager, "errors.save_failed_drawing_preserved").await;
    {
        let _lock = manager.synchronous_lock();
        let path = std::ffi::CString::new(path).unwrap();
        unsafe {
            let input = class(root as *mut lvgl::lv_obj_t, &lvgl::lv_textarea_class);
            lvgl::lv_textarea_set_text(input, path.as_ptr());
        }
    }
    action(manager, 2).await;
    status(manager, "status.saved").await;
    File::read_from_path(vfs, task, path, &mut after_undo)
        .await
        .unwrap();
    assert_eq!(after_undo, saved);
    action(manager, 1).await;
    xila::task::sleep(Duration::from_millis(30)).await;
    manager.send_window_close_request(root).await.unwrap();
    status(manager, "warnings.unsaved_close_again").await;
    manager.send_window_close_request(root).await.unwrap();
    let mut join = core::pin::pin!(handle.join());
    for _ in 0..400 {
        if let Poll::Ready(exit) = poll_fn(|cx| Poll::Ready(join.as_mut().poll(cx))).await {
            assert_eq!(exit, 0);
            assert_eq!(manager.get_window_count().await.unwrap(), 0);
            return;
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Pixel Painter did not close");
}
