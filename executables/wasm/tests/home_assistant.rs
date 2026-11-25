#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
//! Runs the real Home Assistant guest: a first start shows that no
//! configuration is present, and with a configuration and a token it reads
//! the entity states through a scripted HTTP device, shows them, answers a
//! tap on a toggle with a service call followed by a refresh, and shows the
//! error states of a failed service call, a refused token and a missing
//! entity.

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

/// Answers every request according to its path and token, like a small Home
/// Assistant server, and keeps the requests. The first service call
/// succeeds, the next ones fail with 500, and the token `bad-token` is
/// refused with 401.
struct ScriptedHomeAssistant {
    requests: Mutex<Vec<Vec<u8>>>,
    pending: Mutex<Vec<u8>>,
    service_calls: Mutex<usize>,
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
const SERVICE_ERROR: &str =
    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{}";
const UNAUTHORIZED: &str = "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\r\n{}";
const MISSING: &str = "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{}";

fn response_for(request: &[u8], service_call: usize) -> &'static str {
    let text = String::from_utf8_lossy(request);
    let line = text.lines().next().unwrap_or("");
    if text.contains("Bearer bad-token") {
        UNAUTHORIZED
    } else if line.starts_with("POST /api/services/") {
        if service_call == 0 {
            SERVICE
        } else {
            SERVICE_ERROR
        }
    } else if line.starts_with("GET /api/states/light.living_room ") {
        LIGHT
    } else if line.starts_with("GET /api/states/sensor.outdoor_temperature ") {
        SENSOR
    } else if line.starts_with("GET /api/states/media_player.television ") {
        MEDIA_PLAYER
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
        let mut service_calls = self.service_calls.lock().unwrap();
        let is_service = core::str::from_utf8(buffer)
            .unwrap_or("")
            .starts_with("POST /api/services/");
        let response = response_for(buffer, *service_calls);
        if is_service {
            *service_calls += 1;
        }
        drop(service_calls);
        *self.pending.lock().unwrap() = response.as_bytes().to_vec();
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

async fn wait_requests(server: &ScriptedHomeAssistant, count: usize) {
    for _ in 0..1000 {
        if server.requests.lock().unwrap().len() >= count {
            return;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "requests did not reach {count}: {}",
        server.requests.lock().unwrap().len()
    );
}

/// Taps the button whose caption is `caption`.
async fn tap(manager: &Manager, caption: &str) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
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

/// The media control glyphs must exist in the icon font the buttons use.
async fn assert_media_glyphs(manager: &Manager) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let button = find_button(root, "\u{F04C}");
        assert!(!button.is_null(), "no play or pause button");
        let caption = lvgl::lv_obj_get_child(button, 0);
        let font = lvgl::lv_obj_get_style_text_font(caption, 0);
        assert!(!font.is_null(), "the caption has no font");
        for codepoint in [0xF048u32, 0xF04B, 0xF04C, 0xF051] {
            let mut descriptor: lvgl::lv_font_glyph_dsc_t = core::mem::zeroed();
            assert!(
                lvgl::lv_font_get_glyph_dsc(font, &mut descriptor, codepoint, 0),
                "the icon font has no glyph for U+{codepoint:04X}"
            );
        }
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
    panic!("Home Assistant did not exit on close request");
}

#[xila::task::test(task_path=xila::task)]
async fn real_home_assistant_guest_shows_and_controls_the_configured_entities() {
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
            service_calls: Mutex::new(0),
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

    // The runner created the private directory; write the two files. The
    // last entity does not exist on the server, to show the 404 state.
    let directory = "/users/administrator/applications/home_assistant/configuration";
    xila::virtual_file_system::File::write_to_path(
        virtual_file_system,
        task,
        format!("{directory}/configuration.json").as_str(),
        br#"{"url": "http://homeassistant.local:8123", "refresh_seconds": 30,
            "entities": ["light.living_room", "sensor.outdoor_temperature", "media_player.television", "light.missing"]}"#,
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
    // Each configured entity shows its friendly name and its value; the
    // missing one shows its 404 error on its card.
    wait_label(manager, "Living room").await;
    wait_label(manager, "on").await;
    wait_label(manager, "Outdoor temperature").await;
    wait_label(manager, "21.5 °C").await;
    wait_label(manager, "Television").await;
    wait_label(manager, "Channel One - The News").await;
    wait_label(manager, "playing, 40 %").await;
    wait_label(manager, &text("controls.toggle")).await;
    wait_label(manager, &text("errors.not_found")).await;
    assert_media_glyphs(manager).await;
    // One state request per entity for the first refresh.
    wait_requests(home_assistant, 4).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        for (request, path) in requests.iter().zip([
            "GET /api/states/light.living_room ",
            "GET /api/states/sensor.outdoor_temperature ",
            "GET /api/states/media_player.television ",
            "GET /api/states/light.missing ",
        ]) {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(request.starts_with(path), "{request}");
            assert!(request.contains("\r\nHost: homeassistant.local:8123\r\n"));
            assert!(request.contains("\r\nAuthorization: Bearer test-token\r\n"));
        }
    }

    // A tap on the toggle calls the service, then refreshes the states.
    tap(manager, &text("controls.toggle")).await;
    wait_requests(home_assistant, 9).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let post = String::from_utf8(requests[4].clone()).unwrap();
        assert!(
            post.starts_with("POST /api/services/light/toggle HTTP/1.1\r\n"),
            "{post}"
        );
        assert!(post.contains("\r\nAuthorization: Bearer test-token\r\n"));
        assert!(
            post.ends_with("\r\n\r\n{\"entity_id\":\"light.living_room\"}"),
            "{post}"
        );
        // The refresh after the action read every entity again.
        assert!(requests[5].starts_with(b"GET /api/states/light.living_room "));
        assert!(requests[8].starts_with(b"GET /api/states/light.missing "));
    }

    // A service call that fails with 500 shows its error after the refresh.
    tap(manager, &text("controls.toggle")).await;
    wait_requests(home_assistant, 14).await;
    wait_label(manager, "Home Assistant error 500").await;
    close_and_join(manager, second).await;

    // A refused token shows the 401 error, with one request only.
    xila::virtual_file_system::File::write_to_path(
        virtual_file_system,
        task,
        format!("{directory}/token").as_str(),
        b"bad-token\n",
    )
    .await
    .unwrap();
    let before = home_assistant.requests.lock().unwrap().len();
    let third = launch(standard).await.unwrap();
    wait_label(manager, &text("errors.unauthorized")).await;
    close_and_join(manager, third).await;
    let after = home_assistant.requests.lock().unwrap().len();
    assert_eq!(after - before, 1, "one request, not one per entity");
}
