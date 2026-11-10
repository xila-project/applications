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

use core::{ffi::CStr, future::poll_fn, task::Poll, time::Duration};
use xila::graphics::{Manager, lvgl};

fn text(key: &str) -> String {
    localization::text("clock", key)
}

unsafe fn find_label(root: *mut lvgl::lv_obj_t, text: &str, contains: bool) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_label_class) {
            let label = CStr::from_ptr(lvgl::lv_label_get_text(root)).to_bytes();
            if if contains {
                label
                    .windows(text.len())
                    .any(|window| window == text.as_bytes())
            } else {
                label == text.as_bytes()
            } {
                return root;
            }
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find_label(lvgl::lv_obj_get_child(root, index as i32), text, contains);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

async fn wait_label(manager: &Manager, text: &str, contains: bool) {
    for _ in 0..1000 {
        if manager.get_window_count().await.unwrap() != 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let found = {
                let _lock = manager.synchronous_lock();
                unsafe { !find_label(root, text, contains).is_null() }
            };
            if found {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("Clock did not display {text:?}");
}

/// Depth-first search for the first object of `class`.
unsafe fn find_class(
    root: *mut lvgl::lv_obj_t,
    class: *const lvgl::lv_obj_class_t,
    found: &mut Vec<*mut lvgl::lv_obj_t>,
) {
    unsafe {
        if lvgl::lv_obj_has_class(root, class) {
            found.push(root);
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            find_class(lvgl::lv_obj_get_child(root, index as i32), class, found);
        }
    }
}

/// Activates tab `index` and returns its page.
unsafe fn open_tab(root: *mut lvgl::lv_obj_t, index: i32) -> *mut lvgl::lv_obj_t {
    unsafe {
        let mut tabviews = Vec::new();
        find_class(root, &lvgl::lv_tabview_class, &mut tabviews);
        assert_eq!(tabviews.len(), 1);
        let tabs = tabviews[0];
        assert_eq!(lvgl::lv_tabview_get_tab_count(tabs), 4);
        lvgl::lv_tabview_set_active(tabs, index as u32, false);
        lvgl::lv_obj_get_child(lvgl::lv_tabview_get_content(tabs), index)
    }
}

/// Selects roller values in a tab's picker, in on-screen order.
async fn set_rollers(manager: &Manager, tab: i32, values: &[u32]) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let page = open_tab(root, tab);
        let mut rollers = Vec::new();
        find_class(page, &lvgl::lv_roller_class, &mut rollers);
        assert_eq!(rollers.len(), values.len());
        for (roller, value) in rollers.into_iter().zip(values) {
            lvgl::lv_roller_set_selected(roller, *value, false);
        }
    }
}

/// Reads the selected value of a tab's first roller.
async fn first_roller(manager: &Manager, tab: i32) -> u32 {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let page = open_tab(root, tab);
        let mut rollers = Vec::new();
        find_class(page, &lvgl::lv_roller_class, &mut rollers);
        lvgl::lv_roller_get_selected(rollers[0])
    }
}

async fn click(manager: &Manager, tab: Option<i32>, text: &str) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let parent = if let Some(index) = tab {
            open_tab(root, index)
        } else {
            root
        };
        let label = find_label(parent, text, false);
        assert!(!label.is_null(), "missing Clock control {text:?}");
        let button = lvgl::lv_obj_get_parent(label);
        lvgl::lv_obj_send_event(
            button,
            lvgl::lv_event_code_t_LV_EVENT_CLICKED,
            core::ptr::null_mut(),
        );
    }
}

#[xila::task::test(task_path=xila::task)]
async fn real_clock_guest_runs_stopwatch_timer_saves_alarm_and_closes() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let task = context.task_manager.get_current_task_identifier().await;
    let binary = applications::build("xila-clock").unwrap();
    testing::load_file(
        context.virtual_file_system,
        binary,
        "/applications/executables/clock",
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
    // Launch as a login user; the launcher provisions their private app settings.
    let administrator = context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    context
        .virtual_file_system
        .ensure_user_home(task, administrator)
        .await
        .unwrap();
    context
        .task_manager
        .set_user(task, administrator)
        .await
        .unwrap();
    let handle = xila::executable::execute(
        context,
        "/applications/executables/wasm",
        vec!["/applications/executables/clock".into()],
        standard.duplicate().await.unwrap(),
        None,
    )
    .await
    .unwrap();
    wait_label(manager, &text("timer.controls.cancel"), true).await;
    click(manager, Some(2), &text("stopwatch.controls.toggle_pause")).await;
    xila::task::sleep(Duration::from_millis(300)).await;
    click(manager, Some(2), &text("stopwatch.controls.record_lap")).await;
    wait_label(manager, "1: ", true).await;
    click(manager, Some(2), &text("stopwatch.controls.toggle_pause")).await;

    // Select a one-second countdown on the hour, minute and second rollers.
    set_rollers(manager, 3, &[0, 0, 1]).await;
    click(manager, Some(3), &text("timer.controls.start")).await;
    // The alert banner shows catalog messages without their trailing space.
    wait_label(manager, text("alerts.timer_finished").trim(), true).await;
    click(manager, None, &text("alerts.dismiss")).await;

    // Keep the saved occurrence well ahead of now, including around midnight.
    let current_hour = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        / 3600
        % 24;
    let alarm_hour = (current_hour + 12) % 24;
    set_rollers(manager, 1, &[alarm_hour as u32, 0]).await;
    click(manager, Some(1), &text("alarm.controls.save_and_enable")).await;
    wait_label(
        manager,
        &format!("{}{alarm_hour:02}:00 UTC", text("alarm.status.enabled")),
        true,
    )
    .await;
    // Clock shows a change before writing it, so wait for the record.
    let settings = format!(
        "/users/{}/applications/clock/configuration/settings.1",
        administrator.as_u16()
    );
    let mut saved = Vec::new();
    for attempt in 0.. {
        saved.clear();
        match xila::virtual_file_system::File::read_from_path(
            context.virtual_file_system,
            task,
            settings.as_str(),
            &mut saved,
        )
        .await
        {
            Ok(_) if saved.len() == 38 => break,
            _ if attempt < 200 => xila::task::sleep(Duration::from_millis(10)).await,
            result => panic!("settings were not saved: {result:?}, {} bytes", saved.len()),
        }
    }
    assert_eq!(saved.len(), 38, "real WASI persistence record");

    close_and_join(manager, handle).await;
    let reopened = xila::executable::execute(
        context,
        "/applications/executables/wasm",
        vec!["/applications/executables/clock".into()],
        standard,
        None,
    )
    .await
    .unwrap();
    wait_label(manager, &text("timer.controls.cancel"), true).await;
    // No save action: the new guest must load the existing enabled alarm,
    // both in its status and in the hour picker.
    open_alarm_tab(manager).await;
    wait_label(
        manager,
        &format!("{}{alarm_hour:02}:00 UTC", text("alarm.status.enabled")),
        true,
    )
    .await;
    assert_eq!(first_roller(manager, 1).await, alarm_hour as u32);
    close_and_join(manager, reopened).await;
}

async fn open_alarm_tab(manager: &Manager) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        open_tab(root, 1);
    }
}

async fn close_and_join(manager: &Manager, handle: xila::task::JoinHandle<isize>) {
    let root = manager.get_window_identifier(0).await.unwrap();
    manager.send_window_close_request(root).await.unwrap();
    let mut join = core::pin::pin!(handle.join());
    for _ in 0..1000 {
        if let Poll::Ready(status) =
            poll_fn(|context| Poll::Ready(join.as_mut().poll(context))).await
        {
            assert_eq!(status, 0);
            assert_eq!(manager.get_window_count().await.unwrap(), 0);
            return;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("Clock did not exit on close request");
}
