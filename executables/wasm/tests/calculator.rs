#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
#[path = "support/applications.rs"]
mod applications;
testing::instantiate_global_allocator!();
#[path = "support/graphics.rs"]
mod support;

use core::{
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use xila::{
    graphics::{Manager, Window, lvgl},
    task::JoinHandle,
};

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

/// Turns on the angle mode switch, the second child of the display card's
/// first row; off means radians.
async fn select_degrees(manager: &Manager) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let card = lvgl::lv_obj_get_child(root, 0);
        let switch = lvgl::lv_obj_get_child(lvgl::lv_obj_get_child(card, 0), 1);
        assert!(lvgl::lv_obj_check_type(switch, &lvgl::lv_switch_class));
        lvgl::lv_obj_add_state(switch, lvgl::LV_STATE_CHECKED as lvgl::lv_state_t);
        lvgl::lv_obj_send_event(
            switch,
            lvgl::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
            core::ptr::null_mut(),
        );
    }
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

/// Waits for the page markers and for the guest to drain the window event queue.
async fn wait_for_keys(manager: &Manager, keys: &[&str]) {
    for _ in 0..200 {
        let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
        {
            let _lock = manager.synchronous_lock();
            // The identifier points to the live window while the lock is held.
            let ready = unsafe {
                keys.iter().all(|key| visible_keys(root, key).len() == 1)
                    && Window::from_raw(root)
                        .unwrap()
                        .as_ref()
                        .peek_event()
                        .is_none()
            };
            if ready {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Calculator keypad did not become ready with keys {keys:?}");
}

async fn wait_for_display(manager: &Manager, expected: &str) {
    wait_for_display_text(manager, expected, false).await;
}

/// Like [`wait_for_display`], but the whole text must match, so that a
/// result is not mistaken for the start of an error message.
async fn wait_for_display_exactly(manager: &Manager, expected: &str) {
    wait_for_display_text(manager, expected, true).await;
}

async fn wait_for_display_text(manager: &Manager, expected: &str, exact: bool) {
    for _ in 0..200 {
        if manager.get_window_count().await.unwrap() > 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let _lock = manager.synchronous_lock();
            unsafe {
                if label_text(display_label(root)).is_some_and(|text| {
                    if exact {
                        text == expected.as_bytes()
                    } else {
                        text.starts_with(expected.as_bytes())
                    }
                }) {
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
    run_calculator(&[]).await;
}

#[xila::task::test(task_path=xila::task)]
async fn real_calculator_guest_evaluates_buttons_with_small_fuel_slices() {
    run_calculator(&["--instruction-limit", "20000"]).await;
}

/// Evaluates the deepest expression the guest accepts, 31 nested function
/// calls (the parser allows 32 levels), in degrees, which converts the angle
/// at every level. This is the deepest call chain of the bundled guests and
/// needs between 16 and 20 KiB of the guest's 32 KiB stack.
#[xila::task::test(task_path=xila::task)]
async fn real_calculator_guest_evaluates_the_deepest_nested_calls() {
    const DEPTH: usize = 31;
    let (manager, handle) = start_calculator(&[]).await;
    wait_for_display(manager, "0").await;
    select_degrees(manager).await;
    press(manager, "fn").await;
    wait_for_keys(manager, &["123", "C"]).await;
    // One key at a time: the window event queue is bounded.
    let mut expression = String::new();
    for _ in 0..DEPTH {
        press(manager, "sin").await;
        expression.push_str("sin(");
        wait_for_display_exactly(manager, &expression).await;
    }
    press(manager, "123").await;
    wait_for_keys(manager, &["0"]).await;
    press(manager, "0").await;
    expression.push('0');
    wait_for_display_exactly(manager, &expression).await;
    press(manager, "fn").await;
    wait_for_keys(manager, &[")"]).await;
    for _ in 0..DEPTH {
        press(manager, ")").await;
        expression.push(')');
        wait_for_display_exactly(manager, &expression).await;
    }
    press(manager, "123").await;
    wait_for_keys(manager, &["="]).await;
    press(manager, "=").await;
    wait_for_display_exactly(manager, "0").await;
    close_calculator(manager, handle).await;
}

async fn run_calculator(options: &[&str]) {
    let (manager, handle) = start_calculator(options).await;
    wait_for_display(manager, "0").await;
    wait_for_keys(manager, &["="]).await;
    // Keys are real theme-styled buttons, found by their visible label.
    for (key, expected) in [("1", "1"), ("+", "1+"), ("2", "1+2"), ("=", "3")] {
        press(manager, key).await;
        wait_for_display(manager, expected).await;
    }
    // The 320x240 test display uses the compact layout: `fn` pages to the
    // functions and `123` pages back to the digits.
    press(manager, "fn").await;
    // The scientific page has the 123 key and creates its Clear key last.
    // The guest then drains the events queued during creation.
    wait_for_keys(manager, &["123", "C"]).await;
    press(manager, "sqrt").await;
    wait_for_display(manager, "sqrt(").await;
    press(manager, "123").await;
    wait_for_keys(manager, &["7"]).await;
    close_calculator(manager, handle).await;
}

/// Loads the real guest and starts it with `options`.
async fn start_calculator(options: &[&str]) -> (&'static Manager, JoinHandle<isize>) {
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
    let mut arguments: Vec<_> = vec!["--developer-mode".into()];
    arguments.extend(options.iter().map(|&option| option.into()));
    arguments.push("/applications/executables/calculator.wasm".into());
    let handle = xila::executable::execute(
        context,
        "/applications/executables/wasm",
        arguments,
        standard,
        None,
    )
    .await
    .unwrap();
    (manager, handle)
}

/// Requests the window to close and waits for the guest to exit successfully.
async fn close_calculator(manager: &Manager, handle: JoinHandle<isize>) {
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
