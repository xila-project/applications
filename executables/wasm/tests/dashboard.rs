#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
//! Runs the real Dashboard guest: a first start opens the settings, and with
//! a configuration and a token it queries Grafana through a scripted HTTPS
//! device and shows the formatted value. In the settings, the token field
//! starts visible and empty after Replace: Show then edit then Save writes
//! the edit to the token file and the next query carries it, a typed asterisk
//! is kept while the field is shown, an edit made while it is hidden is
//! discarded, the maximum length of the field is restored after the text is
//! set by code, and Cancel leaves the saved token in place.

#[path = "support/applications.rs"]
mod applications;
testing::instantiate_global_allocator!();
#[path = "support/localization.rs"]
mod localization;
#[path = "support/graphics.rs"]
mod support;

use core::{
    ffi::CStr,
    future::poll_fn,
    task::{Context, Poll},
    time::Duration,
};
use std::{ffi::CString, sync::Mutex};
use xila::{
    file_system::{DirectBaseOperations, DirectCharacterDevice, MountOperations, Path, Size},
    graphics::{Manager, lvgl},
};

fn text(key: &str) -> String {
    localization::text("dashboard", key)
}

/// Answers every request with the same response and keeps the requests.
struct ScriptedGrafana {
    requests: Mutex<Vec<Vec<u8>>>,
    pending: Mutex<Vec<u8>>,
}

/// What the token field shows while the token is hidden.
const SECRET_MASK: &str = "********";

const RESPONSE: &str = concat!(
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n",
    r#"{"results": {"P0": {"status": 200, "frames": [{"schema": {"fields": ["#,
    r#"{"name": "Time", "type": "time"}, {"name": "Value", "type": "number"}]}, "#,
    r#""data": {"values": [[1767225600000], [0.25]]}}]}}}"#
);

impl MountOperations for ScriptedGrafana {}
impl DirectCharacterDevice for ScriptedGrafana {}

impl DirectBaseOperations for ScriptedGrafana {
    fn read(
        &self,
        _: &mut Context<'_>,
        buffer: &mut [u8],
        _: Size,
    ) -> Poll<xila::file_system::Result<usize>> {
        let mut pending = self.pending.lock().unwrap();
        let count = buffer.len().min(pending.len());
        buffer[..count].copy_from_slice(&pending[..count]);
        pending.drain(..count);
        Poll::Ready(Ok(count))
    }

    fn write(
        &self,
        _: &mut Context<'_>,
        buffer: &[u8],
        _: Size,
    ) -> Poll<xila::file_system::Result<usize>> {
        self.requests.lock().unwrap().push(buffer.to_vec());
        *self.pending.lock().unwrap() = RESPONSE.as_bytes().to_vec();
        Poll::Ready(Ok(buffer.len()))
    }
}

unsafe fn find_label(root: *mut lvgl::lv_obj_t, text: &str) -> bool {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_label_class)
            && CStr::from_ptr(lvgl::lv_label_get_text(root)).to_bytes() == text.as_bytes()
        {
            return true;
        }
        (0..lvgl::lv_obj_get_child_count(root))
            .any(|index| find_label(lvgl::lv_obj_get_child(root, index as i32), text))
    }
}

/// First button whose caption label holds `caption`.
unsafe fn find_button(root: *mut lvgl::lv_obj_t, caption: &str) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_button_class) {
            for index in 0..lvgl::lv_obj_get_child_count(root) {
                let child = lvgl::lv_obj_get_child(root, index as i32);
                if lvgl::lv_obj_has_class(child, &lvgl::lv_label_class)
                    && CStr::from_ptr(lvgl::lv_label_get_text(child)).to_bytes()
                        == caption.as_bytes()
                {
                    return root;
                }
            }
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find_button(lvgl::lv_obj_get_child(root, index as i32), caption);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

/// First text field in the subtree of `object`.
unsafe fn find_textarea(object: *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(object, &lvgl::lv_textarea_class) {
            return object;
        }
        for index in 0..lvgl::lv_obj_get_child_count(object) {
            let found = find_textarea(lvgl::lv_obj_get_child(object, index as i32));
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

/// First text field that follows the caption label `caption`, in its parent
/// or in the subtree of a following sibling (a row holding the field).
unsafe fn find_field(root: *mut lvgl::lv_obj_t, caption: &str) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_label_class)
            && CStr::from_ptr(lvgl::lv_label_get_text(root)).to_bytes() == caption.as_bytes()
        {
            let parent = lvgl::lv_obj_get_parent(root);
            let count = lvgl::lv_obj_get_child_count(parent);
            let own = (0..count)
                .find(|index| lvgl::lv_obj_get_child(parent, *index as i32) == root)
                .unwrap_or(0);
            for index in own + 1..count {
                let found = find_textarea(lvgl::lv_obj_get_child(parent, index as i32));
                if !found.is_null() {
                    return found;
                }
            }
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find_field(lvgl::lv_obj_get_child(root, index as i32), caption);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

/// Taps the button whose caption is `caption`.
async fn tap(manager: &Manager, caption: &str) {
    let root = manager
        .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
        .await
        .unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let button = find_button(root, caption);
        assert!(!button.is_null(), "no button with caption {caption:?}");
        lvgl::lv_obj_send_event(
            button,
            lvgl::lv_event_code_t_LV_EVENT_CLICKED,
            core::ptr::null_mut(),
        );
    }
}

/// Types `value` into the field that follows the caption `caption`. The text
/// is set as-is, without lifting the maximum length: with the limit set, LVGL
/// sends one insert event per character, and a long value would flood the
/// host event queue and drop later events, so the values typed here stay
/// short on purpose.
async fn type_into(manager: &Manager, caption: &str, value: &str) {
    let root = manager
        .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
        .await
        .unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let field = find_field(root, caption);
        assert!(!field.is_null(), "no field after caption {caption:?}");
        let text = CString::new(value).unwrap();
        lvgl::lv_textarea_set_text(field, text.as_ptr());
        lvgl::lv_obj_send_event(
            field,
            lvgl::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
            core::ptr::null_mut(),
        );
    }
}

/// The text of the field that follows the caption `caption`, if it exists.
async fn field_text(manager: &Manager, caption: &str) -> Option<String> {
    let root = manager
        .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
        .await
        .unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let field = find_field(root, caption);
        if field.is_null() {
            return None;
        }
        Some(
            CStr::from_ptr(lvgl::lv_textarea_get_text(field))
                .to_string_lossy()
                .into_owned(),
        )
    }
}

/// Waits until the field that follows the caption `caption` holds `expected`.
async fn wait_field_text(manager: &Manager, caption: &str, expected: &str) {
    for _ in 0..1000 {
        if manager
            .with_lvgl(|lvgl| manager.get_window_count(lvgl))
            .await
            .unwrap()
            != 0
            && field_text(manager, caption).await.as_deref() == Some(expected)
        {
            return;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("the field after {caption:?} did not show {expected:?}");
}

/// The maximum length of the field that follows the caption `caption`.
async fn field_max_length(manager: &Manager, caption: &str) -> u32 {
    let root = manager
        .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
        .await
        .unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let field = find_field(root, caption);
        assert!(!field.is_null(), "no field after caption {caption:?}");
        lvgl::lv_textarea_get_max_length(field)
    }
}

/// Waits until no label of the window shows `text`.
async fn wait_no_label(manager: &Manager, text: &str) {
    for _ in 0..1000 {
        if manager
            .with_lvgl(|lvgl| manager.get_window_count(lvgl))
            .await
            .unwrap()
            != 0
        {
            let root = manager
                .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
                .await
                .unwrap() as *mut lvgl::lv_obj_t;
            let found = {
                let _lock = manager.synchronous_lock();
                unsafe { find_label(root, text) }
            };
            if !found {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("Dashboard still displays {text:?}");
}

/// Waits until the guest is not inside a refresh: the status line no longer
/// shows the loading text.
async fn wait_idle(manager: &Manager) {
    wait_no_label(manager, &text("header.loading")).await;
}

/// Waits until the scripted server got a request holding `part`.
async fn wait_request_containing(server: &ScriptedGrafana, part: &str) {
    for _ in 0..1000 {
        if server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| String::from_utf8_lossy(request).contains(part))
        {
            return;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("no request contains {part:?}");
}

async fn read_file(
    virtual_file_system: &xila::virtual_file_system::VirtualFileSystem,
    task: xila::task::TaskIdentifier,
    path: &str,
) -> Option<Vec<u8>> {
    let mut buffer = Vec::new();
    match xila::virtual_file_system::File::read_from_path(
        virtual_file_system,
        task,
        path,
        &mut buffer,
    )
    .await
    {
        Ok(()) => Some(buffer),
        Err(_) => None,
    }
}

async fn wait_label(manager: &Manager, text: &str) {
    for _ in 0..1000 {
        if manager
            .with_lvgl(|lvgl| manager.get_window_count(lvgl))
            .await
            .unwrap()
            != 0
        {
            let root = manager
                .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
                .await
                .unwrap() as *mut lvgl::lv_obj_t;
            let found = {
                let _lock = manager.synchronous_lock();
                unsafe { find_label(root, text) }
            };
            if found {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("Dashboard did not display {text:?}");
}

async fn close_and_join(manager: &Manager, handle: xila::task::JoinHandle<isize>) {
    let root = manager
        .with_lvgl(|lvgl| manager.get_window_identifier(lvgl, 0))
        .await
        .unwrap();
    manager
        .with_lvgl(|lvgl| manager.send_window_close_request(lvgl, root))
        .await
        .unwrap();
    let mut join = core::pin::pin!(handle.join());
    for _ in 0..1000 {
        if let Poll::Ready(status) =
            poll_fn(|context| Poll::Ready(join.as_mut().poll(context))).await
        {
            assert_eq!(status, 0);
            assert_eq!(
                manager
                    .with_lvgl(|lvgl| manager.get_window_count(lvgl))
                    .await
                    .unwrap(),
                0
            );
            return;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("Dashboard did not exit on close request");
}

#[xila::task::test(task_path=xila::task)]
async fn real_dashboard_guest_opens_settings_then_shows_a_queried_value() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let virtual_file_system = context.virtual_file_system;
    let task = context.task_manager.get_current_task_identifier().await;
    let binary = applications::build("xila-dashboard").unwrap();
    testing::load_file(
        virtual_file_system,
        binary,
        "/applications/executables/dashboard",
    )
    .await
    .unwrap();
    xila::executable::mount_executables!(
        context,
        virtual_file_system,
        task,
        &[("/applications/executables/wasm", wasm::WasmExecutable)]
    )
    .await
    .unwrap();
    let grafana: &'static ScriptedGrafana = Box::leak(Box::new(ScriptedGrafana {
        requests: Mutex::new(Vec::new()),
        pending: Mutex::new(Vec::new()),
    }));
    virtual_file_system
        .create_directory(task, &"/devices/network")
        .await
        .ok();
    virtual_file_system
        .mount_static(
            task,
            Path::from_str("/devices/network/https_client"),
            xila::virtual_file_system::ItemStatic::CharacterDevice(grafana),
        )
        .await
        .unwrap();

    let administrator = context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    virtual_file_system
        .ensure_user_home(task, administrator)
        .await
        .unwrap();
    context
        .task_manager
        .set_user(task, administrator)
        .await
        .unwrap();
    let launch = |standard| {
        xila::executable::execute(
            context,
            "/applications/executables/wasm",
            vec![
                "--developer-mode".into(),
                "/applications/executables/dashboard".into(),
            ],
            standard,
            None,
        )
    };

    // Without a configuration, the settings open by themselves.
    let first = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("settings.title")).await;
    wait_label(manager, &text("settings.connection")).await;
    close_and_join(manager, first).await;

    // The runner created the private directory; write what Save writes.
    let directory = "/users/administrator/applications/dashboard/configuration";
    xila::virtual_file_system::File::write_to_path(
        virtual_file_system,
        task,
        format!("{directory}/configuration.json").as_str(),
        br#"{"url": "https://grafana.example.com", "data_source": "prometheus",
            "panels": [{"title": "CPU", "expression": "avg(rate(cpu_seconds[5m]))", "unit": "fraction"}]}"#,
    )
    .await
    .unwrap();
    xila::virtual_file_system::File::write_to_path(
        virtual_file_system,
        task,
        format!("{directory}/token").as_str(),
        b"test-token\n",
    )
    .await
    .unwrap();

    let second = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "CPU").await;
    wait_label(manager, "25%").await;
    close_and_join(manager, second).await;

    // In the settings, Replace opens an empty, visible token field: an edit
    // made while it is shown is saved, one made while it is hidden is
    // discarded, and the asterisk typed while it is shown is kept.
    let third = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "CPU").await;
    wait_label(manager, "25%").await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_label(manager, &text("settings.add")).await;
    wait_label(manager, &text("settings.token_set")).await;
    tap(manager, &text("settings.replace")).await;
    // The field starts visible and empty, with its maximum length set.
    wait_field_text(manager, &text("settings.token"), "").await;
    assert_eq!(
        field_max_length(manager, &text("settings.token")).await,
        512
    );
    type_into(manager, &text("settings.token"), "a*b").await;
    wait_field_text(manager, &text("settings.token"), "a*b").await;
    assert_eq!(
        field_max_length(manager, &text("settings.token")).await,
        512
    );
    // Hidden, the field shows the mask and the hint, and an edit is lost.
    tap(manager, &text("settings.hide")).await;
    wait_field_text(manager, &text("settings.token"), SECRET_MASK).await;
    wait_label(manager, &text("settings.token_hint")).await;
    type_into(manager, &text("settings.token"), "hacked").await;
    wait_field_text(manager, &text("settings.token"), SECRET_MASK).await;
    // Shown again, the typed text is back, asterisk included.
    tap(manager, &text("settings.show")).await;
    wait_field_text(manager, &text("settings.token"), "a*b").await;
    tap(manager, &text("settings.save")).await;
    wait_request_containing(grafana, "Bearer a*b").await;
    wait_idle(manager).await;
    assert_eq!(
        read_file(virtual_file_system, task, &format!("{directory}/token"))
            .await
            .expect("token file"),
        b"a*b"
    );
    close_and_join(manager, third).await;

    // Cancel discards the edit: the token file keeps the saved token.
    let fourth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "CPU").await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_label(manager, &text("settings.add")).await;
    tap(manager, &text("settings.replace")).await;
    wait_field_text(manager, &text("settings.token"), "").await;
    type_into(manager, &text("settings.token"), "zzz").await;
    wait_field_text(manager, &text("settings.token"), "zzz").await;
    tap(manager, &text("settings.cancel")).await;
    wait_no_label(manager, &text("settings.title")).await;
    assert_eq!(
        read_file(virtual_file_system, task, &format!("{directory}/token"))
            .await
            .expect("token file"),
        b"a*b"
    );
    close_and_join(manager, fourth).await;

    let requests = grafana.requests.lock().unwrap();
    let request = String::from_utf8(requests[0].clone()).unwrap();
    assert!(request.starts_with("POST /api/ds/query HTTP/1.1\r\nHost: grafana.example.com\r\n"));
    assert!(request.contains("\r\nAuthorization: Bearer test-token\r\n"));
    assert!(request.contains(r#""expr":"avg(rate(cpu_seconds[5m]))","instant":true"#));
}
