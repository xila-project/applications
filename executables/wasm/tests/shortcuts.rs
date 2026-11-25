#![cfg(all(
    feature = "host",
    feature = "graphics",
    feature = "memory_32",
    not(feature = "memory_64")
))]
#![allow(clippy::disallowed_types, reason = "host-only test")]
//! Runs the real Shortcuts guest against a scripted human interface device
//! that records every 8-byte message and answers the GET_STATUS control
//! command. A connected run taps play/pause and the screenshot button of
//! each profile and checks the exact bytes, including the release messages.
//! Busy and short writes are injected to check the retries. A second run
//! without the device node checks the clear message and the disabled
//! buttons.

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
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};
use xila::{
    file_system::{
        ControlCommand, ControlCommandIdentifier, DirectBaseOperations, DirectCharacterDevice,
        IoErrorKind, MountOperations, Path, Size,
    },
    graphics::{Manager, lvgl},
    shared::AnyByLayout,
};

use device::human_interface::{ABI_VERSION, GET_STATUS, INDICATOR_CAPS_LOCK, StatusAbi};

fn text(key: &str) -> String {
    localization::text("shortcuts", key)
}

/// Human interface emulation device that records the messages of every
/// accepted write, can answer busy or accept only part of a write, and
/// reports a configurable connection state.
struct ScriptedHumanInterface {
    messages: Mutex<Vec<[u8; 8]>>,
    /// Writes that fail with WouldBlock before the next accepted one.
    busy_writes: AtomicUsize,
    /// Accept only the first message of each write.
    short_writes: AtomicBool,
    connected: AtomicBool,
    indicators: AtomicU8,
}

impl ScriptedHumanInterface {
    fn connected() -> &'static Self {
        Box::leak(Box::new(Self {
            messages: Mutex::new(Vec::new()),
            busy_writes: AtomicUsize::new(0),
            short_writes: AtomicBool::new(false),
            connected: AtomicBool::new(true),
            indicators: AtomicU8::new(0),
        }))
    }

    fn clear(&self) {
        self.messages.lock().unwrap().clear();
    }

    fn recorded(&self) -> Vec<[u8; 8]> {
        self.messages.lock().unwrap().clone()
    }
}

impl MountOperations for ScriptedHumanInterface {}
impl DirectCharacterDevice for ScriptedHumanInterface {}

impl DirectBaseOperations for ScriptedHumanInterface {
    fn read(
        &self,
        _: &mut Context<'_>,
        _buffer: &mut [u8],
        _: Size,
    ) -> Poll<xila::file_system::Result<usize>> {
        // Reports are write-only.
        Poll::Ready(Err(IoErrorKind::Unsupported))
    }

    fn write(
        &self,
        _: &mut Context<'_>,
        buffer: &[u8],
        _: Size,
    ) -> Poll<xila::file_system::Result<usize>> {
        if self.busy_writes.load(Ordering::Relaxed) > 0 {
            self.busy_writes.fetch_sub(1, Ordering::Relaxed);
            return Poll::Ready(Err(IoErrorKind::WouldBlock));
        }
        let count = if self.short_writes.load(Ordering::Relaxed) {
            buffer.len().min(8)
        } else {
            buffer.len()
        };
        let mut messages = self.messages.lock().unwrap();
        for chunk in buffer[..count].chunks(8) {
            let message: [u8; 8] = chunk.try_into().unwrap();
            messages.push(message);
        }
        Poll::Ready(Ok(count))
    }

    fn control(
        &self,
        _: &mut Context<'_>,
        command: ControlCommandIdentifier,
        input: &AnyByLayout,
        output: &mut AnyByLayout,
    ) -> Poll<xila::file_system::Result<()>> {
        if command == GET_STATUS::IDENTIFIER {
            let request = match GET_STATUS::cast_input(input) {
                Ok(request) => request,
                Err(error) => return Poll::Ready(Err(error)),
            };
            if request.version != ABI_VERSION || request.reserved != 0 {
                return Poll::Ready(Err(IoErrorKind::InvalidArgument));
            }
            let status = match GET_STATUS::cast_output(output) {
                Ok(status) => status,
                Err(error) => return Poll::Ready(Err(error)),
            };
            *status = StatusAbi {
                version: ABI_VERSION,
                connected: self.connected.load(Ordering::Relaxed) as u8,
                indicators: self.indicators.load(Ordering::Relaxed),
                reserved: [0; 4],
            };
            return Poll::Ready(Ok(()));
        }
        Poll::Ready(Err(IoErrorKind::Unsupported))
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

unsafe fn find_class(
    root: *mut lvgl::lv_obj_t,
    class: *const lvgl::lv_obj_class_t,
) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(root, class) {
            return root;
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = find_class(lvgl::lv_obj_get_child(root, index as i32), class);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

unsafe fn dump_labels(root: *mut lvgl::lv_obj_t, out: &mut Vec<String>) {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_label_class) {
            out.push(
                CStr::from_ptr(lvgl::lv_label_get_text(root))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            dump_labels(lvgl::lv_obj_get_child(root, index as i32), out);
        }
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
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let mut labels = Vec::new();
    let _lock = manager.synchronous_lock();
    unsafe { dump_labels(root, &mut labels) };
    panic!("Shortcuts did not display {text:?}; labels: {labels:?}");
}

async fn wait_messages(device: &ScriptedHumanInterface, count: usize) -> Vec<[u8; 8]> {
    for _ in 0..1000 {
        let recorded = device.recorded();
        if recorded.len() >= count {
            return recorded;
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    panic!("messages did not reach {count}: {:?}", device.recorded());
}

/// Waits until the button whose caption is `caption` carries the disabled
/// state, as the guest applies it at the end of its start sequence.
async fn wait_button_disabled(manager: &Manager, caption: &str) -> bool {
    for _ in 0..1000 {
        if manager.get_window_count().await.unwrap() != 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let disabled = {
                let _lock = manager.synchronous_lock();
                unsafe {
                    let button = find_button(root, caption);
                    !button.is_null()
                        && lvgl::lv_obj_has_state(button, lvgl::lv_state_t_LV_STATE_DISABLED)
                }
            };
            if disabled {
                return true;
            }
        }
        xila::task::sleep(Duration::from_millis(10)).await;
    }
    false
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

/// Selects a value on the first dropdown of the window, as the settings tab
/// builds it first, and reports the change to the guest.
async fn select_dropdown(manager: &Manager, value: u32) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let dropdown = find_class(root, &raw const lvgl::lv_dropdown_class);
        assert!(!dropdown.is_null(), "no dropdown in the Shortcuts window");
        lvgl::lv_dropdown_set_selected(dropdown, value);
        lvgl::lv_obj_send_event(
            dropdown,
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
        for codepoint in [0xF026u32, 0xF027, 0xF028, 0xF048, 0xF04C, 0xF04D, 0xF051] {
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
    panic!("Shortcuts did not exit");
}

async fn close_and_join(manager: &Manager, handle: xila::task::JoinHandle<isize>) {
    send_close(manager).await;
    wait_join(manager, handle).await;
}

/// Mounts the scripted device and the guest binary, as the initial task
/// user, before the application runs as the administrator.
async fn prepare(
    context: &'static xila::executable::ExecutableContext,
    device: Option<&'static ScriptedHumanInterface>,
) {
    let task = context.task_manager.get_current_task_identifier().await;
    let virtual_file_system = context.virtual_file_system;
    if let Some(device) = device {
        virtual_file_system
            .create_directory(task, &"/devices")
            .await
            .ok();
        virtual_file_system
            .create_directory(task, &"/devices/human_interface")
            .await
            .ok();
        virtual_file_system
            .mount_static(
                task,
                Path::from_str("/devices/human_interface/emulation"),
                xila::virtual_file_system::ItemStatic::CharacterDevice(device),
            )
            .await
            .unwrap();
    }
    let binary = applications::build("xila-shortcuts").unwrap();
    testing::load_file(
        virtual_file_system,
        binary,
        "/applications/executables/shortcuts",
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
}

/// Runs the application as the administrator and returns its join handle.
async fn launch(
    context: &'static xila::executable::ExecutableContext,
    standard: &xila::executable::Standard<'static>,
) -> xila::task::JoinHandle<isize> {
    let task = context.task_manager.get_current_task_identifier().await;
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
    xila::executable::execute(
        context,
        "/applications/executables/wasm",
        vec![
            "--developer-mode".into(),
            "/applications/executables/shortcuts".into(),
        ],
        standard.duplicate().await.unwrap(),
        None,
    )
    .await
    .unwrap()
}

#[xila::task::test(task_path=xila::task)]
async fn real_shortcuts_guest_sends_media_and_profile_messages() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;

    let device = ScriptedHumanInterface::connected();
    prepare(context, Some(device)).await;
    let handle = launch(context, &standard).await;

    // The guest polls the status at start and shows the connection state.
    wait_label(manager, &text("status.connected")).await;
    assert_media_glyphs(manager).await;

    // Play/pause: the consumer usage, then its release.
    device.clear();
    tap(manager, "\u{F04C}").await;
    let messages = wait_messages(device, 2).await;
    assert_eq!(
        messages,
        [
            [2, 0xCD, 0x00, 0, 0, 0, 0, 0], // play/pause press
            [2, 0, 0, 0, 0, 0, 0, 0],       // release
        ]
    );

    // The screenshot button of the default Windows profile: Win+Shift+S.
    device.clear();
    tap(manager, &text("buttons.screenshot")).await;
    let messages = wait_messages(device, 2).await;
    assert_eq!(
        messages,
        [
            [1, 0x0A, 0x16, 0, 0, 0, 0, 0], // Win+Shift+S press
            [1, 0, 0, 0, 0, 0, 0, 0],       // release
        ]
    );

    // Switch the computer profile to macOS in the settings and tap the
    // screenshot button again: Cmd+Shift+4.
    select_dropdown(manager, 1).await;
    xila::task::sleep(Duration::from_millis(300)).await;
    device.clear();
    tap(manager, &text("buttons.screenshot")).await;
    let messages = wait_messages(device, 2).await;
    assert_eq!(
        messages,
        [
            [1, 0x0A, 0x21, 0, 0, 0, 0, 0], // Cmd+Shift+4 press
            [1, 0, 0, 0, 0, 0, 0, 0],       // release
        ]
    );

    // Linux (GNOME): Print Screen alone.
    select_dropdown(manager, 2).await;
    xila::task::sleep(Duration::from_millis(300)).await;
    device.clear();
    tap(manager, &text("buttons.screenshot")).await;
    let messages = wait_messages(device, 2).await;
    assert_eq!(
        messages,
        [
            [1, 0x00, 0x46, 0, 0, 0, 0, 0], // Print Screen press
            [1, 0, 0, 0, 0, 0, 0, 0],       // release
        ]
    );

    // Busy writes are retried and short writes are completed: the volume up
    // tap still sends exactly its press and its release.
    device.busy_writes.store(3, Ordering::Relaxed);
    device.short_writes.store(true, Ordering::Relaxed);
    device.clear();
    tap(manager, "\u{F028}").await;
    let messages = wait_messages(device, 2).await;
    assert_eq!(
        messages,
        [
            [2, 0xE9, 0x00, 0, 0, 0, 0, 0], // volume up press
            [2, 0, 0, 0, 0, 0, 0, 0],       // release
        ]
    );
    device.short_writes.store(false, Ordering::Relaxed);

    // The caps lock indicator of the computer shows in the status line.
    device
        .indicators
        .store(INDICATOR_CAPS_LOCK, Ordering::Relaxed);
    let expected = format!("{}, {}", text("status.connected"), text("status.caps_lock"));
    wait_label(manager, &expected).await;

    close_and_join(manager, handle).await;
}

#[xila::task::test(task_path=xila::task)]
async fn real_shortcuts_guest_without_device_shows_the_message_and_disables() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;

    // No /devices/human_interface/emulation node is mounted.
    prepare(context, None).await;
    let handle = launch(context, &standard).await;
    wait_label(manager, &text("status.no_device")).await;
    // The play/pause button exists but is disabled.
    assert!(
        wait_button_disabled(manager, "\u{F04C}").await,
        "the buttons must be disabled without a computer"
    );
    close_and_join(manager, handle).await;
}
