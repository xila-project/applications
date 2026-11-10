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
#[path = "support/graphics.rs"]
mod support;

use core::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use xila::graphics::{Manager, lvgl};

/// Text of a label, or `None` for a null object.
unsafe fn label_text(label: *mut lvgl::lv_obj_t) -> Option<&'static [u8]> {
    if label.is_null() {
        return None;
    }
    unsafe { Some(core::ffi::CStr::from_ptr(lvgl::lv_label_get_text(label)).to_bytes()) }
}

/// The result label: the display card's third child, after the angle-mode
/// row and the history label.
unsafe fn display_label(root: *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t {
    unsafe {
        let card = lvgl::lv_obj_get_child(root, 0);
        if card.is_null() {
            return core::ptr::null_mut();
        }
        lvgl::lv_obj_get_child(card, 2)
    }
}

/// Visible keys whose label reads `text`; keypad pages are the window's
/// second child and each key is a button holding one label.
unsafe fn visible_keys(root: *mut lvgl::lv_obj_t, text: &str) -> Vec<*mut lvgl::lv_obj_t> {
    unsafe {
        let keypad = lvgl::lv_obj_get_child(root, 1);
        assert!(!keypad.is_null());
        let mut matches = Vec::new();
        for page_index in 0..lvgl::lv_obj_get_child_count(keypad) {
            let page = lvgl::lv_obj_get_child(keypad, page_index as i32);
            if lvgl::lv_obj_has_flag(page, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN) {
                continue;
            }
            for key_index in 0..lvgl::lv_obj_get_child_count(page) {
                let key = lvgl::lv_obj_get_child(page, key_index as i32);
                if label_text(lvgl::lv_obj_get_child(key, 0)) == Some(text.as_bytes()) {
                    matches.push(key);
                }
            }
        }
        matches
    }
}

unsafe fn find_key(root: *mut lvgl::lv_obj_t, text: &str) -> *mut lvgl::lv_obj_t {
    let matches = unsafe { visible_keys(root, text) };
    assert_eq!(matches.len(), 1, "key {text:?}");
    matches[0]
}

async fn press(manager: &Manager, key: &str) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        lvgl::lv_obj_send_event(
            find_key(root, key),
            lvgl::lv_event_code_t_LV_EVENT_CLICKED,
            core::ptr::null_mut(),
        );
    }
}

/// Waits until the compact keypad shows the page containing `key`.
async fn wait_for_key(manager: &Manager, key: &str) {
    for _ in 0..200 {
        let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
        {
            let _lock = manager.synchronous_lock();
            if unsafe { visible_keys(root, key) }.len() == 1 {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Calculator did not show key {key:?}");
}

async fn wait_for_display(manager: &Manager, expected: &str) {
    for _ in 0..200 {
        if manager.get_window_count().await.unwrap() > 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let _lock = manager.synchronous_lock();
            unsafe {
                if label_text(display_label(root))
                    .is_some_and(|text| text.starts_with(expected.as_bytes()))
                {
                    return;
                }
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Calculator did not show {expected:?}");
}

#[xila::task::test(task_path=xila::task)]
async fn real_calculator_guest_evaluates_buttons_and_closes_on_wasmi() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let task = context.task_manager.get_current_task_identifier().await;
    let binary = applications::build("xila-calculator").unwrap();
    testing::load_file(
        context.virtual_file_system,
        binary,
        "/applications/executables/calculator.wasm",
    )
    .await
    .unwrap();
    xila::executable::mount_executables!(
        context,
        context.virtual_file_system,
        task,
        &[("/applications/executables/wasm", wasm::WasmExecutable)]
    )
    .await
    .unwrap();
    let handle = xila::executable::execute(
        context,
        "/applications/executables/wasm",
        vec!["/applications/executables/calculator.wasm".into()],
        standard,
        None,
    )
    .await
    .unwrap();
    wait_for_display(manager, "0").await;
    // Keys are real theme-styled buttons, found by their visible label.
    for (key, expected) in [("1", "1"), ("+", "1+"), ("2", "1+2"), ("=", "3")] {
        press(manager, key).await;
        wait_for_display(manager, expected).await;
    }
    // The 320x240 test display uses the compact layout: `fn` pages to the
    // functions and `123` pages back to the digits.
    press(manager, "fn").await;
    wait_for_key(manager, "sqrt").await;
    press(manager, "sqrt").await;
    wait_for_display(manager, "sqrt(").await;
    press(manager, "123").await;
    wait_for_key(manager, "7").await;
    let root = manager.get_window_identifier(0).await.unwrap();
    manager.send_window_close_request(root).await.unwrap();
    let mut join = core::pin::pin!(handle.join());
    for _ in 0..200 {
        let result = poll_fn(|context| Poll::Ready(join.as_mut().poll(context))).await;
        if let Poll::Ready(status) = result {
            assert_eq!(status, 0);
            assert_eq!(manager.get_window_count().await.unwrap(), 0);
            return;
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Calculator did not exit on close request");
}
