#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
//! Runs the real Home Assistant guest against a scripted HTTP device that
//! answers like a small Home Assistant server: entity states, service calls,
//! the login flow, the token endpoint and the template endpoint.
//!
//! It checks that a first start opens the settings when there is no
//! configuration, that with a configuration and a long-lived token it reads
//! the entity states, answers a tap on a toggle with a service call followed
//! by a refresh, and shows the error states of a failed service call, a
//! refused token and a missing entity. It then signs in with a username and
//! password through the scripted login flow, picks an entity from the server
//! through the template endpoint, saves, and shows the cards, renewing the
//! access token through the refresh token when it expires. A revoked refresh
//! token opens the settings again, signed out. With a slow device, it checks
//! that a tap made during a refresh is ignored, and that a close request made
//! during a refresh still exits the application.

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
    localization::text("home_assistant", key)
}

fn response(head_status: &str, body: &str) -> Vec<u8> {
    std::format!("{head_status}\r\nContent-Type: application/json\r\n\r\n{body}").into_bytes()
}

const LIGHT: &str = include_str!("../../home_assistant/tests/fixtures/light.json");
const SWITCH: &str = include_str!("../../home_assistant/tests/fixtures/switch.json");
const SENSOR: &str = include_str!("../../home_assistant/tests/fixtures/sensor.json");
const MEDIA_PLAYER: &str = include_str!("../../home_assistant/tests/fixtures/media_player.json");
const SERVICE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n[]";
const SERVICE_ERROR: &str =
    "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\n\r\n{}";
const UNAUTHORIZED: &str = "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\r\n{}";
const MISSING: &str = "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\n\r\n{}";
const FORM_INIT: &str = r#"{"flow_id":"f1","handler":["homeassistant",null],"type":"form","step_id":"init","data_schema":[],"errors":{}}"#;
const FORM_MFA: &str = r#"{"flow_id":"f1","handler":["homeassistant",null],"type":"form","step_id":"mfa","errors":{}}"#;
const FORM_INVALID_AUTH: &str =
    r#"{"flow_id":"f1","type":"form","step_id":"init","errors":{"base":"invalid_auth"}}"#;
const CREATE_ENTRY: &str = r#"{"flow_id":"f1","handler":["homeassistant",null],"type":"create_entry","version":1,"title":"Alice","result":"auth-code-1"}"#;
const INVALID_GRANT: &str = "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\n\r\n{\"error\":\"invalid_grant\"}";
const REVOKED_OK: &str = "HTTP/1.1 200 OK\r\n\r\n";
const AREAS_ANSWER: &str = "living_room|Living room\nkitchen|Kitchen\n";
const LIVING_ROOM_ANSWER: &str = "light.living_room|Living room\nswitch.pump|Pump\n";
const NO_AREA_ANSWER: &str = "sensor.stray|Stray sensor\n";

/// Answers every request according to its path and token, like a small Home
/// Assistant server, and keeps the requests. The first service call
/// succeeds, the next ones fail with 500, and the token `bad-token` is
/// refused with 401. The login flow answers `f1`; the token endpoint issues
/// `access-1`, `access-2`, ... with the configured `expires_in`, and refuses
/// the refresh token `revoked` with 400. Every answer can be delayed, so the
/// guest is caught inside a refresh.
struct ScriptedHomeAssistant {
    requests: Mutex<Vec<Vec<u8>>>,
    pending: Mutex<Vec<u8>>,
    service_calls: Mutex<usize>,
    /// The answer waiting to be served, with its earliest serve time.
    queued: Mutex<Option<(Vec<u8>, std::time::Instant)>>,
    delay: Mutex<Duration>,
    /// How many access tokens were issued.
    access_tokens: Mutex<usize>,
    /// The `expires_in` answered by the token endpoint.
    token_expires_in: Mutex<u32>,
}

impl ScriptedHomeAssistant {
    fn token_answer(&self, with_refresh_token: bool) -> Vec<u8> {
        let count = {
            let mut tokens = self.access_tokens.lock().unwrap();
            *tokens += 1;
            *tokens
        };
        let expires_in = *self.token_expires_in.lock().unwrap();
        let body = if with_refresh_token {
            std::format!(
                "{{\"access_token\":\"access-{count}\",\"expires_in\":{expires_in},\"refresh_token\":\"refresh-1\",\"token_type\":\"Bearer\"}}"
            )
        } else {
            std::format!(
                "{{\"access_token\":\"access-{count}\",\"expires_in\":{expires_in},\"token_type\":\"Bearer\"}}"
            )
        };
        response("HTTP/1.1 200 OK", &body)
    }

    fn template_answer(&self, request: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(request);
        let answer = if text.contains("area_entities('living_room')") {
            LIVING_ROOM_ANSWER
        } else if text.contains("not in in_areas") {
            NO_AREA_ANSWER
        } else {
            AREAS_ANSWER
        };
        std::format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n{answer}").into_bytes()
    }

    fn answer(&self, request: &[u8], service_call: usize) -> Vec<u8> {
        let text = String::from_utf8_lossy(request);
        let line = text.lines().next().unwrap_or("");
        if text.contains("Bearer bad-token") {
            UNAUTHORIZED.as_bytes().to_vec()
        } else if line.starts_with("POST /auth/login_flow/f1 ") {
            if text.contains("\"code\":") {
                if text.contains("123456") {
                    response("HTTP/1.1 200 OK", CREATE_ENTRY)
                } else {
                    response(
                        "HTTP/1.1 200 OK",
                        r#"{"flow_id":"f1","type":"form","step_id":"mfa","errors":{"base":"invalid_code"}}"#,
                    )
                }
            } else if text.contains("\"password\":\"wrong\"") {
                response("HTTP/1.1 200 OK", FORM_INVALID_AUTH)
            } else if text.contains("\"password\":\"mfa\"") {
                response("HTTP/1.1 200 OK", FORM_MFA)
            } else {
                response("HTTP/1.1 200 OK", CREATE_ENTRY)
            }
        } else if line.starts_with("POST /auth/login_flow ") {
            response("HTTP/1.1 200 OK", FORM_INIT)
        } else if line.starts_with("POST /auth/token ") {
            if text.contains("grant_type=authorization_code") {
                self.token_answer(true)
            } else if text.contains("refresh_token=revoked") {
                INVALID_GRANT.as_bytes().to_vec()
            } else {
                self.token_answer(false)
            }
        } else if line.starts_with("POST /auth/revoke ") {
            REVOKED_OK.as_bytes().to_vec()
        } else if line.starts_with("POST /api/template ") {
            self.template_answer(request)
        } else if line.starts_with("POST /api/services/") {
            if service_call == 0 {
                SERVICE.as_bytes().to_vec()
            } else {
                SERVICE_ERROR.as_bytes().to_vec()
            }
        } else if line.starts_with("GET /api/states/light.living_room ") {
            response("HTTP/1.1 200 OK", LIGHT)
        } else if line.starts_with("GET /api/states/switch.pump ") {
            response("HTTP/1.1 200 OK", SWITCH)
        } else if line.starts_with("GET /api/states/sensor.outdoor_temperature ") {
            response("HTTP/1.1 200 OK", SENSOR)
        } else if line.starts_with("GET /api/states/media_player.television ") {
            response("HTTP/1.1 200 OK", MEDIA_PLAYER)
        } else {
            MISSING.as_bytes().to_vec()
        }
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
        if pending.is_empty() {
            let mut queued = self.queued.lock().unwrap();
            let due = matches!(&*queued, Some((_, at)) if std::time::Instant::now() >= *at);
            if due {
                if let Some((response, _)) = queued.take() {
                    *pending = response;
                }
            } else if queued.is_some() {
                // The answer is not due yet: report no data for now.
                return Poll::Pending;
            } else {
                return Poll::Ready(Ok(0));
            }
        }
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
        let response = self.answer(buffer, *service_calls);
        if is_service {
            *service_calls += 1;
        }
        drop(service_calls);
        let delay = *self.delay.lock().unwrap();
        *self.queued.lock().unwrap() = Some((response, std::time::Instant::now() + delay));
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

/// First text field that follows the caption label `caption` in its parent.
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
                let child = lvgl::lv_obj_get_child(parent, index as i32);
                if lvgl::lv_obj_has_class(child, &lvgl::lv_textarea_class) {
                    return child;
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

/// The switch of the row whose label holds `row_text`.
unsafe fn find_switch(root: *mut lvgl::lv_obj_t, row_text: &str) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_switch_class) {
            let parent = lvgl::lv_obj_get_parent(root);
            for index in 0..lvgl::lv_obj_get_child_count(parent) {
                let child = lvgl::lv_obj_get_child(parent, index as i32);
                if lvgl::lv_obj_has_class(child, &lvgl::lv_label_class)
                    && CStr::from_ptr(lvgl::lv_label_get_text(child)).to_bytes()
                        == row_text.as_bytes()
                {
                    return root;
                }
            }
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find_switch(lvgl::lv_obj_get_child(root, index as i32), row_text);
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

/// Waits until the guest is not inside a refresh: the status line no longer
/// shows the loading text.
async fn wait_idle(manager: &Manager) {
    let loading = text("header.loading");
    for _ in 0..1000 {
        if manager.get_window_count().await.unwrap() != 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let busy = {
                let _lock = manager.synchronous_lock();
                unsafe { find_label(root, &loading) }
            };
            if !busy {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("Home Assistant did not become idle");
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

/// Types `value` into the field that follows the caption `caption`.
async fn type_into(manager: &Manager, caption: &str, value: &str) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
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

/// Sets the switch of the row labelled `row_text` and reports the change.
async fn set_switch(manager: &Manager, row_text: &str, checked: bool) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let switch = find_switch(root, row_text);
        assert!(!switch.is_null(), "no switch in row {row_text:?}");
        if checked {
            lvgl::lv_obj_add_state(switch, lvgl::LV_STATE_CHECKED);
        } else {
            lvgl::lv_obj_remove_state(switch, lvgl::LV_STATE_CHECKED);
        }
        lvgl::lv_obj_send_event(
            switch,
            lvgl::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
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

async fn send_close(manager: &Manager) {
    let root = manager.get_window_identifier(0).await.unwrap();
    manager.send_window_close_request(root).await.unwrap();
}

async fn wait_join(manager: &Manager, handle: xila::task::JoinHandle<isize>) {
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
    panic!("Home Assistant did not exit");
}

async fn close_and_join(manager: &Manager, handle: xila::task::JoinHandle<isize>) {
    send_close(manager).await;
    wait_join(manager, handle).await;
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
            queued: Mutex::new(None),
            delay: Mutex::new(Duration::ZERO),
            access_tokens: Mutex::new(0),
            token_expires_in: Mutex::new(1800),
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
    let directory = "/users/administrator/applications/home_assistant/configuration";
    async fn write_file(
        virtual_file_system: &xila::virtual_file_system::VirtualFileSystem,
        task: xila::task::TaskIdentifier,
        path: &str,
        contents: &[u8],
    ) {
        xila::virtual_file_system::File::write_to_path(virtual_file_system, task, path, contents)
            .await
            .unwrap();
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
    let configuration = br#"{"url": "http://homeassistant.local:8123", "refresh_seconds": 30,
        "entities": ["light.living_room", "sensor.outdoor_temperature", "media_player.television", "light.missing"]}"#;

    // Without a configuration, the settings open by themselves.
    let first = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("settings.title")).await;
    wait_label(manager, &text("main.not_configured")).await;
    close_and_join(manager, first).await;

    // The runner created the private directory; write the two files. The
    // last entity does not exist on the server, to show the 404 state.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/configuration.json"),
        configuration,
    )
    .await;
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"test-token\n",
    )
    .await;

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
    wait_idle(manager).await;
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
    wait_idle(manager).await;
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
    wait_idle(manager).await;
    close_and_join(manager, second).await;

    // A refused token shows the 401 error, with one request only.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"bad-token\n",
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let third = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("errors.unauthorized")).await;
    wait_idle(manager).await;
    close_and_join(manager, third).await;
    let after = home_assistant.requests.lock().unwrap().len();
    assert_eq!(after - before, 1, "one request, not one per entity");

    // A slow server: a tap made during a refresh is drained and ignored,
    // and a close request made during a refresh still exits the guest.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"test-token\n",
    )
    .await;
    *home_assistant.delay.lock().unwrap() = Duration::from_millis(400);
    let before = home_assistant.requests.lock().unwrap().len();
    let fourth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    // The first state request is written, its answer is still delayed: the
    // guest is inside the refresh.
    wait_requests(home_assistant, before + 1).await;
    tap(manager, &text("controls.toggle")).await;
    wait_requests(home_assistant, before + 4).await;
    wait_label(manager, "Living room").await;
    wait_idle(manager).await;
    {
        // The tap landed during the refresh: no service call was made.
        let requests = home_assistant.requests.lock().unwrap();
        for request in &requests[before..] {
            assert!(
                request.starts_with(b"GET /api/states/"),
                "unexpected request: {}",
                String::from_utf8_lossy(request)
            );
        }
    }
    // A close request during the next refresh is kept, not drained.
    tap(manager, "\u{F021}").await;
    wait_requests(home_assistant, before + 5).await;
    send_close(manager).await;
    wait_join(manager, fourth).await;
    *home_assistant.delay.lock().unwrap() = Duration::ZERO;

    // Sign in with a username and password: the settings open by themselves
    // when there are no credentials. The token endpoint answers a short
    // `expires_in`, so the first requests renew the access token.
    *home_assistant.token_expires_in.lock().unwrap() = 30;
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b" \n",
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let fifth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("settings.title")).await;
    wait_label(manager, &text("settings.not_signed_in")).await;
    // A wrong password shows the form error.
    tap(manager, &text("settings.sign_in")).await;
    wait_label(manager, &text("settings.username")).await;
    type_into(manager, &text("settings.username"), "alice").await;
    type_into(manager, &text("settings.password"), "wrong").await;
    tap(manager, &text("settings.sign_in")).await;
    wait_label(manager, &text("settings.reason.invalid_auth")).await;
    // The right password signs in.
    type_into(manager, &text("settings.password"), "s3cret").await;
    tap(manager, &text("settings.sign_in")).await;
    wait_label(manager, "Signed in as Alice").await;
    wait_idle(manager).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let start = String::from_utf8(requests[before].clone()).unwrap();
        assert!(
            start.starts_with("POST /auth/login_flow HTTP/1.1\r\n"),
            "{start}"
        );
        assert!(!start.contains("Authorization:"), "{start}");
        assert!(start.contains("\r\nContent-Type: application/json\r\n"));
        assert!(
            start.ends_with(
                "\r\n\r\n{\"client_id\":\"https://xila.dev/applications/home-assistant/\",\"handler\":[\"homeassistant\",null],\"redirect_uri\":\"https://xila.dev/applications/home-assistant/\"}"
            ),
            "{start}"
        );
        let submit = String::from_utf8(requests[before + 1].clone()).unwrap();
        assert!(
            submit.starts_with("POST /auth/login_flow/f1 HTTP/1.1\r\n"),
            "{submit}"
        );
        assert!(!submit.contains("Authorization:"), "{submit}");
        assert!(
            submit.ends_with("\r\n\r\n{\"client_id\":\"https://xila.dev/applications/home-assistant/\",\"username\":\"alice\",\"password\":\"wrong\"}"),
            "{submit}"
        );
        let submit = String::from_utf8(requests[before + 2].clone()).unwrap();
        assert!(
            submit.ends_with("\"username\":\"alice\",\"password\":\"s3cret\"}"),
            "{submit}"
        );
        let token = String::from_utf8(requests[before + 3].clone()).unwrap();
        assert!(
            token.starts_with("POST /auth/token HTTP/1.1\r\n"),
            "{token}"
        );
        assert!(!token.contains("Authorization:"), "{token}");
        assert!(token.contains("\r\nContent-Type: application/x-www-form-urlencoded\r\n"));
        assert!(
            token.ends_with("\r\n\r\ngrant_type=authorization_code&code=auth-code-1&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"),
            "{token}"
        );
    }
    *home_assistant.token_expires_in.lock().unwrap() = 1800;

    // Pick an entity from the server: areas first, then the entities of the
    // area, each queried through the template endpoint with the access token.
    tap(manager, &text("settings.add_from_server")).await;
    wait_label(manager, &text("settings.picker_title")).await;
    // The access token from the sign-in expires within the margin, so the
    // first request renews it through the refresh token, then asks for the
    // areas with the new access token.
    wait_requests(home_assistant, before + 5).await;
    wait_label(manager, "Living room (living_room)").await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let refresh = String::from_utf8(requests[before + 4].clone()).unwrap();
        assert!(
            refresh.starts_with("POST /auth/token HTTP/1.1\r\n"),
            "{refresh}"
        );
        assert!(
            refresh.ends_with("\r\n\r\ngrant_type=refresh_token&refresh_token=refresh-1&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"),
            "{refresh}"
        );
        let template = String::from_utf8(requests[before + 5].clone()).unwrap();
        assert!(
            template.starts_with("POST /api/template HTTP/1.1\r\n"),
            "{template}"
        );
        assert!(
            template.contains("\r\nAuthorization: Bearer access-2\r\n"),
            "{template}"
        );
        assert!(template.contains("{% for area in areas() %}"), "{template}");
    }
    tap(manager, "Living room (living_room)").await;
    wait_label(manager, "Pump (switch.pump)").await;
    wait_requests(home_assistant, before + 6).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let template = String::from_utf8(requests[before + 6].clone()).unwrap();
        assert!(
            template.contains("area_entities('living_room')"),
            "{template}"
        );
        assert!(
            template.contains("\r\nAuthorization: Bearer access-2\r\n"),
            "{template}"
        );
    }
    set_switch(manager, "Pump (switch.pump)", true).await;
    tap(manager, &text("settings.back")).await;
    wait_label(manager, "switch.pump").await;

    // Save: the configuration and the credentials are written, the token file
    // is removed, and the cards show. The session is reloaded from the files,
    // so it has no access token in memory yet: the first refresh renews it
    // through the refresh token, then reads the states with it.
    tap(manager, &text("settings.save")).await;
    wait_label(manager, "Living room").await;
    wait_label(manager, "Garden pump").await;
    wait_requests(home_assistant, before + 8 + 5).await;
    wait_idle(manager).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let refresh = String::from_utf8(requests[before + 7].clone()).unwrap();
        assert!(
            refresh.starts_with("POST /auth/token HTTP/1.1\r\n"),
            "{refresh}"
        );
        assert!(
            refresh.ends_with("\r\n\r\ngrant_type=refresh_token&refresh_token=refresh-1&client_id=https%3A%2F%2Fxila.dev%2Fapplications%2Fhome-assistant%2F"),
            "{refresh}"
        );
        for request in &requests[before + 8..before + 13] {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(request.starts_with("GET /api/states/"), "{request}");
            assert!(
                request.contains("\r\nAuthorization: Bearer access-3\r\n"),
                "{request}"
            );
        }
        assert!(requests[before + 8].starts_with(b"GET /api/states/light.living_room "));
        assert!(requests[before + 12].starts_with(b"GET /api/states/switch.pump "));
    }
    // The credentials file holds the refresh token and the name; the token
    // file is gone. Neither holds the password.
    {
        let credentials = read_file(
            virtual_file_system,
            task,
            &format!("{directory}/credentials.json"),
        )
        .await
        .expect("credentials file");
        let credentials = String::from_utf8(credentials).unwrap();
        assert!(
            credentials.contains("\"refresh_token\":\"refresh-1\""),
            "{credentials}"
        );
        assert!(credentials.contains("\"name\":\"Alice\""), "{credentials}");
        assert!(!credentials.contains("s3cret"), "{credentials}");
        assert!(
            read_file(virtual_file_system, task, &format!("{directory}/token"))
                .await
                .is_none(),
            "the token file is removed"
        );
    }

    // A manual refresh reads the states again with the same access token.
    tap(manager, "\u{F021}").await;
    wait_requests(home_assistant, before + 13 + 5).await;
    wait_idle(manager).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        for request in &requests[before + 13..before + 18] {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(request.starts_with("GET /api/states/"), "{request}");
            assert!(
                request.contains("\r\nAuthorization: Bearer access-3\r\n"),
                "{request}"
            );
        }
    }
    close_and_join(manager, fifth).await;

    // A revoked refresh token: the refresh fails with 400, the credentials
    // are forgotten and the settings open, signed out, with one request only.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/credentials.json"),
        br#"{"refresh_token":"revoked","name":"Alice"}"#,
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let sixth = launch(standard).await.unwrap();
    wait_label(manager, &text("settings.title")).await;
    wait_label(manager, &text("settings.not_signed_in")).await;
    wait_requests(home_assistant, before + 1).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        assert_eq!(
            requests.len() - before,
            1,
            "one request, not one per entity"
        );
        let refresh = String::from_utf8(requests[before].clone()).unwrap();
        assert!(
            refresh.starts_with("POST /auth/token HTTP/1.1\r\n"),
            "{refresh}"
        );
        assert!(
            refresh.contains("grant_type=refresh_token&refresh_token=revoked"),
            "{refresh}"
        );
    }
    assert!(
        read_file(
            virtual_file_system,
            task,
            &format!("{directory}/credentials.json")
        )
        .await
        .is_none(),
        "the credentials file is removed after the revocation"
    );
    close_and_join(manager, sixth).await;
}
