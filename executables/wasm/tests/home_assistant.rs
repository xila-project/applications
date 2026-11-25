#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
//! Runs the real Home Assistant guest against a scripted HTTP device that
//! answers like a small Home Assistant server: entity states, service calls
//! and the template endpoint.
//!
//! It checks that a first start opens the settings when there is no
//! configuration, and again when there is a configuration but no token, and
//! that Cancel then leaves the main screen with its cards. With an empty
//! token file, the auto-opened settings show the token field visible and
//! empty: a token typed there is saved and sent as the bearer token. In the settings it
//! imports a token from a file of the scripted file system and checks the
//! exact errors of an invalid, an empty and a missing file, then saves and
//! sees the cards with the imported token. It answers a tap on a toggle with
//! a service call followed by a refresh, and shows the error states of a
//! failed service call, a refused token and a missing entity. The token field
//! shows a mask until Show is pressed; an edit made while it is hidden is
//! discarded, a typed asterisk is kept while it is shown, Show then edit then
//! Save writes the edit to the token file, and Cancel leaves the saved token
//! in place. The area picker lists the areas and the entities of an area and
//! shows its 401 error on its own page when a call fails while it is open.
//! With a slow device, it checks that a tap made during a refresh is ignored,
//! and that a close request made during a refresh or a picker call still
//! exits the application.

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
const AREAS_ANSWER: &str = "living_room|Living room\nkitchen|Kitchen\n";
const LIVING_ROOM_ANSWER: &str = "light.living_room|Living room\nswitch.pump|Pump\n";
const NO_AREA_ANSWER: &str = "sensor.stray|Stray sensor\n";
/// What the token field shows while the token is hidden.
const SECRET_MASK: &str = "********";

/// Answers every request according to its path and token, like a small Home
/// Assistant server, and keeps the requests. The first service call
/// succeeds, the next ones fail with 500. The token `bad-token` is refused
/// with 401, and `user-token` is refused on the template endpoint only, like
/// a non-administrator account. Every answer can be delayed, so the guest is
/// caught inside a refresh.
struct ScriptedHomeAssistant {
    requests: Mutex<Vec<Vec<u8>>>,
    pending: Mutex<Vec<u8>>,
    service_calls: Mutex<usize>,
    /// The answer waiting to be served, with its earliest serve time.
    queued: Mutex<Option<(Vec<u8>, std::time::Instant)>>,
    delay: Mutex<Duration>,
}

impl ScriptedHomeAssistant {
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
        } else if line.starts_with("POST /api/template ") {
            if text.contains("Bearer user-token")
                || (text.contains("area_entities(") && text.contains("Bearer area-token"))
            {
                // The template endpoint needs an administrator account.
                UNAUTHORIZED.as_bytes().to_vec()
            } else {
                self.template_answer(request)
            }
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

/// Waits until no label of the window shows `text`.
async fn wait_no_label(manager: &Manager, text: &str) {
    for _ in 0..1000 {
        if manager.get_window_count().await.unwrap() != 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
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
    panic!("Home Assistant still displays {text:?}");
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

/// The text of the field that follows the caption `caption`.
async fn field_text(manager: &Manager, caption: &str) -> String {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let field = find_field(root, caption);
        assert!(!field.is_null(), "no field after caption {caption:?}");
        CStr::from_ptr(lvgl::lv_textarea_get_text(field))
            .to_string_lossy()
            .into_owned()
    }
}

/// Waits until the field that follows the caption `caption` holds `expected`.
async fn wait_field_text(manager: &Manager, caption: &str, expected: &str) {
    for _ in 0..1000 {
        if manager.get_window_count().await.unwrap() != 0
            && field_text(manager, caption).await == expected
        {
            return;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("the field after {caption:?} did not show {expected:?}");
}

/// Waits until the guest finished building the settings screen. The guest
/// yields between host calls while it builds, so a label of the first widget
/// alone is not enough: wait for the add buttons, built last.
async fn wait_settings_ready(manager: &Manager) {
    wait_label(manager, &text("settings.add_from_server")).await;
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

/// Types `value` into the field that follows the caption `caption`. The text
/// is set as-is, without lifting the maximum length: with the limit set, LVGL
/// sends one insert event per character, and a long value would flood the
/// host event queue and drop later events, so the values typed here stay
/// short on purpose.
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

/// The maximum length of the field that follows the caption `caption`.
async fn field_max_length(manager: &Manager, caption: &str) -> u32 {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let field = find_field(root, caption);
        assert!(!field.is_null(), "no field after caption {caption:?}");
        lvgl::lv_textarea_get_max_length(field)
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

/// Waits for the guest to exit. The host sends the close request only once,
/// so no resend here: a dropped one would be a bug.
async fn wait_join(manager: &Manager, handle: xila::task::JoinHandle<isize>) {
    let mut join = core::pin::pin!(handle.join());
    for _ in 0..100 {
        if let Poll::Ready(status) =
            poll_fn(|context| Poll::Ready(join.as_mut().poll(context))).await
        {
            assert_eq!(status, 0);
            assert_eq!(manager.get_window_count().await.unwrap(), 0);
            return;
        }
        xila::task::sleep(Duration::from_millis(100)).await;
    }
    panic!("Home Assistant did not exit");
}

/// Sends one close request and waits for the exit.
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

    // The scripted token files sit at short paths, so the tests can type the
    // paths without flooding the host event queue. They are written as the
    // initial user and stay readable by the guest.
    for (path, contents) in [
        ("/b.txt", &b"two words\n"[..]),
        ("/e.txt", &b"\n"[..]),
        ("/t.txt", &b"imported-token\n"[..]),
    ] {
        xila::virtual_file_system::File::write_to_path(virtual_file_system, task, path, contents)
            .await
            .unwrap();
    }
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

    // The runner created the private directory; write the configuration
    // file. The last entity does not exist on the server, to show the 404
    // state. Without a token, the settings open again by themselves.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/configuration.json"),
        configuration,
    )
    .await;
    let second = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;

    // Cancel leaves the main screen with its cards built from the
    // configuration; the settings open again from the header button. The
    // cards already exist behind the settings, so wait for the settings
    // page itself to be gone before tapping the header button: a tap sent
    // while the guest still processes the Cancel would reopen the settings
    // over the typed text.
    tap(manager, &text("settings.cancel")).await;
    wait_no_label(manager, &text("settings.title")).await;
    wait_label(manager, "light.living_room").await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;

    // Import a token from a file of the scripted file system. The invalid,
    // the empty and the missing files each show their own error. The paths
    // stay short because type_into sets the text without lifting the field
    // limit, and a long value would flood the host event queue.
    let import_path = text("settings.import_path");
    let import = text("settings.import");
    type_into(manager, &import_path, "/b.txt").await;
    tap(manager, &import).await;
    wait_label(manager, &text("errors.import_invalid")).await;
    type_into(manager, &import_path, "/e.txt").await;
    tap(manager, &import).await;
    wait_label(manager, &text("errors.import_empty")).await;
    type_into(manager, &import_path, "/x.txt").await;
    tap(manager, &import).await;
    wait_label(manager, &text("errors.import_missing")).await;
    type_into(manager, &import_path, "/t.txt").await;
    tap(manager, &import).await;
    wait_label(manager, &text("settings.imported")).await;
    // The imported token is hidden behind the mask, and the source file is
    // unchanged.
    wait_field_text(manager, &text("settings.token"), SECRET_MASK).await;
    assert_eq!(
        read_file(virtual_file_system, task, "/t.txt")
            .await
            .unwrap(),
        b"imported-token\n"
    );

    // Save: the configuration and the token are written, and the cards show.
    tap(manager, &text("settings.save")).await;
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
    // One state request per entity for the first refresh, with the imported
    // token in the header.
    wait_requests(home_assistant, 4).await;
    wait_idle(manager).await;
    {
        let token_file = read_file(virtual_file_system, task, &format!("{directory}/token"))
            .await
            .expect("token file");
        assert_eq!(token_file, b"imported-token");
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
            assert!(request.contains("\r\nAuthorization: Bearer imported-token\r\n"));
        }
    }

    // A tap on the toggle calls the service, then refreshes the states.
    wait_idle(manager).await;
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
        assert!(post.contains("\r\nAuthorization: Bearer imported-token\r\n"));
        assert!(
            post.ends_with("\r\n\r\n{\"entity_id\":\"light.living_room\"}"),
            "{post}"
        );
        // The refresh after the action read every entity again.
        assert!(requests[5].starts_with(b"GET /api/states/light.living_room "));
        assert!(requests[8].starts_with(b"GET /api/states/light.missing "));
    }

    // A service call that fails with 500 shows its error after the refresh.
    wait_idle(manager).await;
    tap(manager, &text("controls.toggle")).await;
    wait_requests(home_assistant, 14).await;
    wait_label(manager, "Home Assistant error 500").await;
    wait_idle(manager).await;
    close_and_join(manager, second).await;

    // An empty token file opens the settings by themselves, with the token
    // field visible and empty: a token typed there is saved and used.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"",
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let typed = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    // The field starts visible: the toggle reads Hide, not Show.
    wait_label(manager, &text("settings.hide")).await;
    wait_field_text(manager, &text("settings.token"), "").await;
    type_into(manager, &text("settings.token"), "typed-token").await;
    wait_field_text(manager, &text("settings.token"), "typed-token").await;
    tap(manager, &text("settings.save")).await;
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 4).await;
    wait_idle(manager).await;
    assert_eq!(
        read_file(virtual_file_system, task, &format!("{directory}/token"))
            .await
            .expect("token file"),
        b"typed-token"
    );
    {
        let requests = home_assistant.requests.lock().unwrap();
        for request in &requests[before..] {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(request.starts_with("GET /api/states/"), "{request}");
            assert!(
                request.contains("\r\nAuthorization: Bearer typed-token\r\n"),
                "{request}"
            );
        }
    }
    close_and_join(manager, typed).await;

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
    wait_idle(manager).await;
    tap(manager, "\u{F021}").await;
    wait_requests(home_assistant, before + 5).await;
    send_close(manager).await;
    wait_join(manager, fourth).await;
    *home_assistant.delay.lock().unwrap() = Duration::ZERO;

    // The token field edits: the saved token is hidden behind the mask, an
    // edit made while it is hidden is discarded, Show reveals the token with
    // the maximum length restored, a typed asterisk is kept while it is
    // shown, and Save writes the edit to the token file.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"test-token\n",
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let fifth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 4).await;
    wait_idle(manager).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let request = String::from_utf8(requests[before].clone()).unwrap();
        assert!(request.contains("\r\nAuthorization: Bearer test-token\r\n"));
    }
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    // The saved token is hidden behind the mask.
    wait_field_text(manager, &text("settings.token"), SECRET_MASK).await;
    // An edit made while the field is hidden is discarded.
    type_into(manager, &text("settings.token"), "hacked").await;
    wait_field_text(manager, &text("settings.token"), SECRET_MASK).await;
    // Show reveals the saved token, not the discarded edit, and the maximum
    // length of the field is restored after the bounded text set.
    tap(manager, &text("settings.show")).await;
    wait_field_text(manager, &text("settings.token"), "test-token").await;
    assert_eq!(
        field_max_length(manager, &text("settings.token")).await,
        512
    );
    // A typed asterisk is kept while the field is shown.
    type_into(manager, &text("settings.token"), "a*b").await;
    wait_field_text(manager, &text("settings.token"), "a*b").await;
    tap(manager, &text("settings.hide")).await;
    wait_field_text(manager, &text("settings.token"), SECRET_MASK).await;
    assert_eq!(
        field_max_length(manager, &text("settings.token")).await,
        512
    );
    // Save writes the edited token; the refresh after it uses the new one.
    tap(manager, &text("settings.save")).await;
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 8).await;
    wait_idle(manager).await;
    {
        assert_eq!(
            read_file(virtual_file_system, task, &format!("{directory}/token"))
                .await
                .unwrap(),
            b"a*b"
        );
        let requests = home_assistant.requests.lock().unwrap();
        for request in &requests[before + 4..before + 8] {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(request.starts_with("GET /api/states/"), "{request}");
            assert!(
                request.contains("\r\nAuthorization: Bearer a*b\r\n"),
                "{request}"
            );
        }
    }
    close_and_join(manager, fifth).await;

    // Cancel keeps the saved token: an edit that is not saved never reaches
    // the token file, and no request carries it.
    let before = home_assistant.requests.lock().unwrap().len();
    let sixth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 4).await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    tap(manager, &text("settings.show")).await;
    wait_field_text(manager, &text("settings.token"), "a*b").await;
    type_into(manager, &text("settings.token"), "zzz").await;
    wait_field_text(manager, &text("settings.token"), "zzz").await;
    tap(manager, &text("settings.cancel")).await;
    wait_label(manager, "Living room").await;
    assert_eq!(
        read_file(virtual_file_system, task, &format!("{directory}/token"))
            .await
            .unwrap(),
        b"a*b"
    );
    {
        let requests = home_assistant.requests.lock().unwrap();
        for request in &requests[before..] {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(!request.contains("Bearer zzz"), "{request}");
        }
    }
    close_and_join(manager, sixth).await;

    // The area picker lists the areas of the server, then the entities of one
    // area, and saving adds the chosen entity. All of it with the saved,
    // edited token.
    let before = home_assistant.requests.lock().unwrap().len();
    let seventh = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 4).await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    tap(manager, &text("settings.add_from_server")).await;
    wait_label(manager, &text("settings.picker_title")).await;
    wait_requests(home_assistant, before + 5).await;
    wait_label(manager, "Living room (living_room)").await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let template = String::from_utf8(requests[before + 4].clone()).unwrap();
        assert!(
            template.starts_with("POST /api/template HTTP/1.1\r\n"),
            "{template}"
        );
        assert!(template.contains("\r\nAuthorization: Bearer a*b\r\n"));
        assert!(template.contains("{% for area in areas() %}"), "{template}");
    }
    // Wait for the areas call to finish (the loading line is gone) before
    // tapping: a tap sent while the guest still loads would be drained.
    wait_no_label(manager, &text("settings.loading")).await;
    tap(manager, "Living room (living_room)").await;
    wait_label(manager, "Pump (switch.pump)").await;
    wait_no_label(manager, &text("settings.loading")).await;
    wait_requests(home_assistant, before + 6).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let template = String::from_utf8(requests[before + 5].clone()).unwrap();
        assert!(
            template.contains("area_entities('living_room')"),
            "{template}"
        );
        assert!(template.contains("\r\nAuthorization: Bearer a*b\r\n"));
    }
    set_switch(manager, "Pump (switch.pump)", true).await;
    tap(manager, &text("settings.back")).await;
    wait_label(manager, "switch.pump").await;
    tap(manager, &text("settings.save")).await;
    wait_label(manager, "Garden pump").await;
    wait_requests(home_assistant, before + 6 + 5).await;
    wait_idle(manager).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        for request in &requests[before + 6..before + 11] {
            let request = String::from_utf8(request.clone()).unwrap();
            assert!(request.starts_with("GET /api/states/"), "{request}");
            assert!(request.contains("\r\nAuthorization: Bearer a*b\r\n"));
        }
        assert!(requests[before + 10].starts_with(b"GET /api/states/switch.pump "));
    }
    close_and_join(manager, seventh).await;

    // A 401 on an area's entity list while the picker is open: the error shows
    // on the picker page, which stays open.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"area-token\n",
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let eighth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 4).await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    tap(manager, &text("settings.add_from_server")).await;
    wait_label(manager, &text("settings.picker_title")).await;
    wait_requests(home_assistant, before + 5).await;
    wait_label(manager, "Living room (living_room)").await;
    wait_no_label(manager, &text("settings.loading")).await;
    tap(manager, "Living room (living_room)").await;
    wait_requests(home_assistant, before + 7).await;
    // The error is on the picker page, and the picker is still open.
    wait_label(manager, &text("errors.admin_needed")).await;
    wait_label(manager, &text("settings.picker_title")).await;
    close_and_join(manager, eighth).await;

    // A 401 on the areas call itself: the picker closes and the settings page
    // shows the error.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"user-token\n",
    )
    .await;
    let before = home_assistant.requests.lock().unwrap().len();
    let ninth = launch(standard.duplicate().await.unwrap()).await.unwrap();
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 5).await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    tap(manager, &text("settings.add_from_server")).await;
    wait_requests(home_assistant, before + 6).await;
    wait_label(manager, &text("errors.admin_needed")).await;
    {
        let requests = home_assistant.requests.lock().unwrap();
        let template = String::from_utf8(requests[before + 5].clone()).unwrap();
        assert!(
            template.starts_with("POST /api/template HTTP/1.1\r\n"),
            "{template}"
        );
        assert!(template.contains("\r\nAuthorization: Bearer user-token\r\n"));
    }
    close_and_join(manager, ninth).await;

    // A close request during a picker call still exits the guest.
    write_file(
        virtual_file_system,
        task,
        &format!("{directory}/token"),
        b"test-token\n",
    )
    .await;
    *home_assistant.delay.lock().unwrap() = Duration::from_millis(400);
    let before = home_assistant.requests.lock().unwrap().len();
    let tenth = launch(standard).await.unwrap();
    wait_label(manager, "Living room").await;
    wait_requests(home_assistant, before + 4).await;
    wait_idle(manager).await;
    tap(manager, "\u{F013}").await;
    wait_label(manager, &text("settings.title")).await;
    wait_settings_ready(manager).await;
    tap(manager, &text("settings.add_from_server")).await;
    // The areas call is written, its answer is still delayed: the guest is
    // inside the picker call.
    wait_requests(home_assistant, before + 5).await;
    send_close(manager).await;
    wait_join(manager, tenth).await;
    *home_assistant.delay.lock().unwrap() = Duration::ZERO;
}
