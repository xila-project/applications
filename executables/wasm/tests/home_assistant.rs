#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
//! Runs the real Home Assistant guest: a first start shows that no
//! configuration is present, and with a configuration and a token it reads
//! the entity states through a scripted HTTP device and shows them.

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
    localization::text("home_assistant", key)
}

/// Answers every request according to its path, like a small Home Assistant
/// server, and keeps the requests.
struct ScriptedHomeAssistant {
    requests: Mutex<Vec<Vec<u8>>>,
    pending: Mutex<Vec<u8>>,
}

const LIGHT: &str = concat!(
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n",
    include_str!("../../home_assistant/tests/fixtures/light.json")
);
const SENSOR: &str = concat!(
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n",
    include_str!("../../home_assistant/tests/fixtures/sensor.json")
);
const MEDIA_PLAYER: &str = concat!(
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n",
    include_str!("../../home_assistant/tests/fixtures/media_player.json")
);
const SERVICE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[]";
const MISSING: &str = "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{}";

fn response_for(request: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(request);
    let line = text.lines().next().unwrap_or("");
    if line.starts_with("GET /api/states/light.living_room ") {
        LIGHT
    } else if line.starts_with("GET /api/states/sensor.outdoor_temperature ") {
        SENSOR
    } else if line.starts_with("GET /api/states/media_player.television ") {
        MEDIA_PLAYER
    } else if line.starts_with("POST /api/services/") {
        SERVICE
    } else {
        MISSING
    }
}

impl MountOperations for ScriptedHomeAssistant {}
impl DirectCharacterDevice for ScriptedHomeAssistant {}

impl DirectBaseOperations for ScriptedHomeAssistant {
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
        *self.pending.lock().unwrap() = response_for(buffer).as_bytes().to_vec();
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
    panic!("Home Assistant did not display {text:?}");
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
    panic!("Home Assistant did not exit on close request");
}

#[xila::task::test(task_path=xila::task)]
async fn real_home_assistant_guest_shows_the_configured_entities() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let virtual_file_system = context.virtual_file_system;
    let task = context.task_manager.get_current_task_identifier().await;
    let binary = applications::build("xila-home-assistant").unwrap();
    testing::load_file(
        virtual_file_system,
        binary,
        "/applications/executables/home_assistant",
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
    let home_assistant: &'static ScriptedHomeAssistant =
        Box::leak(Box::new(ScriptedHomeAssistant {
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
            Path::from_str("/devices/network/http_client"),
            xila::virtual_file_system::ItemStatic::CharacterDevice(home_assistant),
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
                "/applications/executables/home_assistant".into(),
            ],
            standard,
            None,
        )
    };

    // Without a configuration, the notice says that there is none.
    let first = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("main.not_configured")).await;
    close_and_join(manager, first).await;

    // The runner created the private directory; write the two files.
    let directory = "/users/administrator/applications/home_assistant/configuration";
    xila::virtual_file_system::File::write_to_path(
        virtual_file_system,
        task,
        format!("{directory}/configuration.json").as_str(),
        br#"{"url": "http://homeassistant.local:8123", "refresh_seconds": 30,
            "entities": ["light.living_room", "sensor.outdoor_temperature", "media_player.television"]}"#,
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
    // Each configured entity shows its friendly name and its value.
    wait_label(manager, "Living room").await;
    wait_label(manager, "on").await;
    wait_label(manager, "Outdoor temperature").await;
    wait_label(manager, "21.5 °C").await;
    wait_label(manager, "Television").await;
    wait_label(manager, "Channel One - The News").await;
    wait_label(manager, "playing, 40 %").await;
    // The controls of each domain show.
    wait_label(manager, &text("controls.toggle")).await;
    close_and_join(manager, second).await;

    let requests = home_assistant.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        3,
        "one state request per entity per refresh"
    );
    for (request, path) in requests.iter().zip([
        "GET /api/states/light.living_room ",
        "GET /api/states/sensor.outdoor_temperature ",
        "GET /api/states/media_player.television ",
    ]) {
        let request = String::from_utf8(request.clone()).unwrap();
        assert!(request.starts_with(path), "{request}");
        assert!(request.contains("\r\nHost: homeassistant.local:8123\r\n"));
        assert!(request.contains("\r\nAuthorization: Bearer test-token\r\n"));
    }
}
