#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
//! Runs the real Dashboard guest: a first start opens the settings, and with
//! a configuration and a token it queries Grafana through a scripted HTTPS
//! device and shows the formatted value.

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
use std::sync::Mutex;
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

async fn wait_label(manager: &Manager, text: &str) {
    for _ in 0..1000 {
        if manager.get_window_count().await.unwrap() != 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
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

    let second = launch(standard).await.unwrap();
    wait_label(manager, "CPU").await;
    wait_label(manager, "25%").await;
    close_and_join(manager, second).await;

    let requests = grafana.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "one query request per refresh");
    let request = String::from_utf8(requests[0].clone()).unwrap();
    assert!(request.starts_with("POST /api/ds/query HTTP/1.1\r\nHost: grafana.example.com\r\n"));
    assert!(request.contains("\r\nAuthorization: Bearer test-token\r\n"));
    assert!(request.contains(r#""expr":"avg(rate(cpu_seconds[5m]))","instant":true"#));
}
