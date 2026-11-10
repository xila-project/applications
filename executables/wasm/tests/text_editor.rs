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

use core::time::Duration;
use xila::{
    file_system::{AccessFlags, CreateFlags, Flags},
    graphics::{Manager, Window, lvgl},
    virtual_file_system::{File, SynchronousDirectory},
};

unsafe fn find(
    root: *mut lvgl::lv_obj_t,
    class: *const lvgl::lv_obj_class_t,
    expected: Option<&str>,
) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_get_class(root) == class {
            let matches = expected.is_none_or(|expected| {
                core::ffi::CStr::from_ptr(lvgl::lv_label_get_text(root))
                    .to_string_lossy()
                    .contains(expected)
            });
            if matches {
                return root;
            }
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find(lvgl::lv_obj_get_child(root, index as i32), class, expected);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

async fn wait_label(manager: &Manager, expected: &str) {
    for _ in 0..400 {
        if manager.get_window_count().await.unwrap() > 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let _lock = manager.synchronous_lock();
            if unsafe { !find(root, &lvgl::lv_label_class, Some(expected)).is_null() } {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Text Editor did not show {expected:?}");
}

async fn action(manager: &Manager, key: &str) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let mut window = Window::from_raw(root).unwrap();
        while window.as_mut().pop_event().is_some() {}
        let label = find(
            root,
            &lvgl::lv_label_class,
            Some(&localization::text("text_editor", key)),
        );
        assert!(!label.is_null(), "Missing action {key}");
        let button = lvgl::lv_obj_get_parent(label);
        assert_eq!(
            lvgl::lv_obj_get_class(button),
            &lvgl::lv_button_class as *const _
        );
        lvgl::lv_obj_send_event(
            button,
            lvgl::lv_event_code_t_LV_EVENT_CLICKED,
            core::ptr::null_mut(),
        );
    }
}

async fn confirm_picker(manager: &Manager) {
    for _ in 0..400 {
        let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
        {
            let _lock = manager.synchronous_lock();
            unsafe {
                let picker = find(root, xila::graphics::get_file_picker_class(), None);
                if !picker.is_null()
                    && lvgl::lv_obj_get_class(picker) == xila::graphics::get_file_picker_class()
                    && !lvgl::lv_obj_has_flag(picker, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN)
                {
                    let path = find(picker, &lvgl::lv_textarea_class, None);
                    // READY is the same completion used by physical Enter.
                    lvgl::lv_obj_send_event(
                        path,
                        lvgl::lv_event_code_t_LV_EVENT_READY,
                        core::ptr::null_mut(),
                    );
                    return;
                }
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Text Editor did not open the system picker");
}

#[xila::task::test(task_path = xila::task)]
async fn guest_opens_pages_edits_and_saves_large_file() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let task = context.task_manager.get_current_task_identifier().await;
    let vfs = context.virtual_file_system;
    vfs.ensure_user_application_directories(task, xila::users::UserIdentifier::ROOT, "text_editor")
        .await
        .unwrap();
    let path = "/users/0/applications/text_editor/data/document.txt";
    let original = "a".repeat(9000);
    let mut source = File::open(
        vfs,
        task,
        path,
        Flags::new(AccessFlags::Write, Some(CreateFlags::CREATE_TRUNCATE), None),
    )
    .await
    .unwrap();
    let mut remaining = original.as_bytes();
    while !remaining.is_empty() {
        let written = source.write(remaining).await.unwrap();
        assert_ne!(written, 0);
        remaining = &remaining[written..];
    }
    source.close(vfs).await.unwrap();
    let mut source_contents = Vec::new();
    File::read_from_path(vfs, task, path, &mut source_contents)
        .await
        .unwrap();
    assert_eq!(source_contents, original.as_bytes());
    let binary = applications::build("xila-text-editor").unwrap();
    testing::load_file(vfs, binary, "/applications/executables/text_editor")
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
        vec!["/applications/executables/text_editor".into()],
        standard,
        None,
    )
    .await
    .unwrap();
    wait_label(
        manager,
        &localization::text("text_editor", "file_path.picker_instructions"),
    )
    .await;
    let path_text = std::ffi::CString::new(path).unwrap();
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    {
        let _lock = manager.synchronous_lock();
        unsafe {
            let path_input = find(root, &lvgl::lv_textarea_class, None);
            lvgl::lv_textarea_set_text(path_input, path_text.as_ptr());
        }
    }
    action(manager, "controls.open_file").await;
    confirm_picker(manager).await;
    wait_label(manager, &localization::text("text_editor", "status.opened")).await;
    wait_label(manager, "4096 / 9000").await;
    xila::task::sleep(Duration::from_millis(30)).await;
    action(manager, "navigation.next_page").await;
    wait_label(manager, "8192 / 9000").await;
    xila::task::sleep(Duration::from_millis(30)).await;
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    {
        let _lock = manager.synchronous_lock();
        unsafe {
            let path_input = find(root, &lvgl::lv_textarea_class, None);
            // The document is the other textarea (the path stays in details).
            let mut text = core::ptr::null_mut();
            for index in 0..lvgl::lv_obj_get_child_count(root) {
                let child = lvgl::lv_obj_get_child(root, index as i32);
                if lvgl::lv_obj_get_class(child) == &lvgl::lv_textarea_class && child != path_input
                {
                    text = child;
                }
            }
            assert!(!text.is_null());
            lvgl::lv_textarea_set_text(text, c"edited".as_ptr());
        }
    }
    action(manager, "controls.new_file").await;
    wait_label(
        manager,
        &localization::text("text_editor", "warnings.unsaved_new_or_open_again"),
    )
    .await;
    action(manager, "controls.save_as").await;
    confirm_picker(manager).await;
    wait_label(manager, &localization::text("text_editor", "status.saved")).await;
    xila::task::sleep(Duration::from_millis(30)).await;
    let mut saved = Vec::new();
    File::read_from_path(vfs, task, path, &mut saved)
        .await
        .unwrap();
    assert_eq!(
        saved,
        format!("{}edited{}", "a".repeat(4096), "a".repeat(9000 - 8192)).as_bytes()
    );
    action(manager, "controls.choose_folder").await;
    confirm_picker(manager).await;
    wait_label(
        manager,
        &localization::text("text_editor", "status.folder_selected"),
    )
    .await;
    let window = manager.get_window_identifier(0).await.unwrap();
    manager.send_window_close_request(window).await.unwrap();
    let exit = handle.join().await;
    assert_eq!(exit, 0);
    let directory =
        SynchronousDirectory::open(vfs, task, "/users/0/applications/text_editor/data").unwrap();
    let names: Vec<_> = directory.map(|entry| entry.name).collect();
    assert!(names.iter().any(|name| name == "document.txt"));
    assert!(
        !names
            .iter()
            .any(|name| name.starts_with(".xila-text-editor-"))
    );
}
