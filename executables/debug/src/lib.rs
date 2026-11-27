#![no_std]

extern crate alloc;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use core::num::NonZeroUsize;
use portable_atomic::{AtomicBool, Ordering};

use debug_protocol::{
    InputState, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, Request, ScreenshotMetadata,
    ScreenshotPixelFormat,
};
use xila::{
    executable::{self, ExecutableTrait, Standard},
    file_system::AccessFlags,
    graphics::{self, EventKind, Key, State},
    synchronization::Arc,
    task::{self, TaskIdentifier},
    virtual_file_system::{File, VirtualFileSystem},
};

mod inspection;

pub struct DebugExecutable;

impl ExecutableTrait for DebugExecutable {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
        Box::pin(async move { run(standard, arguments).await })
    }
}

async fn create_debug_window(
    graphics: &'static graphics::Manager,
) -> Result<graphics::OwnedWindow, NonZeroUsize> {
    create_debug_window_with(graphics, build_debug_window).await
}

async fn create_debug_window_with(
    graphics: &'static graphics::Manager,
    build: impl FnOnce(
        &mut graphics::OwnedWindow,
        &mut graphics::GraphicsGuard<'_>,
    ) -> Result<(), NonZeroUsize>,
) -> Result<graphics::OwnedWindow, NonZeroUsize> {
    let mut guard = graphics.lock().await;
    let mut window = guard
        .with_lvgl(|lvgl| graphics.create_window(lvgl).map_err(|_| code(1)))
        .map_err(|_| code(1))??;
    if let Err(error) = build(&mut window, &mut guard) {
        // Delete the window in the current session instead of letting
        // `OwnedWindow::drop` queue it behind the held lock.
        let _ = guard.with_lvgl(|lvgl| window.delete(lvgl));
        return Err(error);
    }
    Ok(window)
}

fn build_debug_window(
    window: &mut graphics::OwnedWindow,
    guard: &mut graphics::GraphicsGuard<'_>,
) -> Result<(), NonZeroUsize> {
    guard
        .with_lvgl(|lvgl| window.set_icon(lvgl, "Db", WINDOW_ICON_COLOR))
        .map_err(|_| code(1))?;
    let label = unsafe { graphics::lvgl::lv_label_create(window.as_object()) };
    if label.is_null() {
        return Err(code(1));
    }
    unsafe {
        graphics::lvgl::lv_label_set_text(
            label,
            c"Debug agent is ready. Use xila-debug from the host.".as_ptr(),
        );
        graphics::lvgl::lv_obj_center(label);
    }
    Ok(())
}

async fn run(standard: Standard<'static>, arguments: Vec<String>) -> Result<(), NonZeroUsize> {
    let context = standard.context;
    let graphics = graphics::ffi_manager();
    let background = arguments.iter().any(|argument| argument == "--background");
    let mut window = if background {
        None
    } else {
        Some(create_debug_window(graphics).await?)
    };

    let vfs = context.virtual_file_system;
    let task_manager = context.task_manager;
    let task = task_manager.get_current_task_identifier().await;
    let stopping = Arc::new(AtomicBool::new(false));
    let task_stopping = stopping.clone();
    let (service_handle, _) = task_manager
        .spawn(
            task,
            "Debug agent transport",
            None,
            async move |service_task| match serve_agent(vfs, context, service_task, task_stopping)
                .await
            {
                Ok(TransportEnd::Stopped) => {}
                Ok(TransportEnd::ServedElsewhere) => log::information!(
                    "Debug agent transport is already served by another Debug instance"
                ),
                Err(error) => log::error!("Debug agent transport stopped: {error}"),
            },
        )
        .await
        .map_err(|_| code(1))?;

    let mut should_close = false;
    loop {
        if let Some(window) = window.as_mut() {
            let mut guard = graphics.lock().await;
            should_close = guard
                .with_lvgl(|lvgl| {
                    let mut close = false;
                    while let Some(event) = window.pop_event(lvgl) {
                        if matches!(event.code, EventKind::Delete | EventKind::CloseRequested)
                            && event.is_target(window.as_object())
                        {
                            close = true;
                            break;
                        }
                    }
                    close
                })
                .unwrap_or(false);
        }
        if should_close {
            stopping.store(true, Ordering::Release);
            service_handle.join().await;
            return Ok(());
        }
        task::sleep(core::time::Duration::from_millis(50)).await;
    }
}

/// Path of the debug transport device, which one opening serves at a time.
const TRANSPORT_PATH: &str = "/devices/graphics/debug/transport";

/// How the transport task ended without an error.
#[derive(Debug, PartialEq, Eq)]
enum TransportEnd {
    /// The window closed and the transport was released.
    Stopped,
    /// Another instance, typically the `--background` one, already holds the
    /// transport and answers `xila-debug`; this instance only shows its window.
    ServedElsewhere,
}

async fn serve_agent(
    vfs: &'static VirtualFileSystem,
    context: &'static xila::executable::ExecutableContext,
    task: TaskIdentifier,
    stopping: Arc<AtomicBool>,
) -> Result<TransportEnd, String> {
    let mut transport =
        match File::open(vfs, task, TRANSPORT_PATH, AccessFlags::READ_WRITE.into()).await {
            Ok(transport) => transport,
            // The transport is opened exclusively: `ResourceBusy` is another
            // opening, not a failure of this one.
            Err(xila::virtual_file_system::IoErrorKind::ResourceBusy) => {
                return Ok(TransportEnd::ServedElsewhere);
            }
            Err(error) => return Err(error.to_string()),
        };
    let mut request_buffer = alloc::vec![0; MAX_REQUEST_BYTES + 1];
    loop {
        if stopping.load(Ordering::Acquire) {
            break;
        }
        let length = match transport
            .as_synchronous_file_mut()
            .read(&mut request_buffer)
        {
            Ok(length) => length,
            // `WouldBlock` is the readiness case: the transport has nothing to
            // read yet. `ResourceBusy` (exclusive access) is a real failure.
            Err(xila::virtual_file_system::IoErrorKind::WouldBlock) => {
                task::sleep(core::time::Duration::from_millis(10)).await;
                continue;
            }
            Err(error) => return Err(error.to_string()),
        };
        let mut response = match parse_request(&request_buffer[..length]) {
            Ok(request) => dispatch(request, context).await,
            Err((code, message)) => DispatchResponse::json(error_response(code, &message)),
        };
        let mut response_bytes =
            serde_json::to_vec(&response.value).map_err(|error| error.to_string())?;
        if response_bytes.len() + 1 > MAX_RESPONSE_BYTES {
            response_bytes = serde_json::to_vec(&error_response(
                "response_too_large",
                "Response exceeds the 64 KiB limit; request a smaller UI tree.",
            ))
            .map_err(|error| error.to_string())?;
            response.payload = None;
        }
        response_bytes.push(b'\n');
        write_all(&mut transport, &response_bytes).await?;
        if let Some(payload) = response.payload {
            write_all(&mut transport, &payload).await?;
        }
    }

    transport
        .close(vfs)
        .await
        .map_err(|error| error.to_string())?;
    Ok(TransportEnd::Stopped)
}

async fn write_all(file: &mut File<'_>, bytes: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        let written = file
            .write(&bytes[offset..])
            .await
            .map_err(|error| error.to_string())?;
        if written == 0 {
            return Err("Transport returned a zero-length write.".into());
        }
        offset += written;
    }
    Ok(())
}

fn parse_request(bytes: &[u8]) -> Result<Request, (&'static str, String)> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err((
            "request_too_large",
            "Request exceeds the 16 KiB limit.".into(),
        ));
    }
    let request: Request = serde_json::from_slice(bytes)
        .map_err(|_| ("invalid_json", "Request is not valid JSON.".into()))?;
    debug_protocol::validate_request(&request)
        .map_err(|error| (error.code(), "Request validation failed.".into()))?;
    Ok(request)
}

struct DispatchResponse {
    value: serde_json::Value,
    payload: Option<Vec<u8>>,
}

impl DispatchResponse {
    fn json(value: serde_json::Value) -> Self {
        Self {
            value,
            payload: None,
        }
    }
}

async fn dispatch(
    request: Request,
    context: &'static xila::executable::ExecutableContext,
) -> DispatchResponse {
    let graphics_manager = graphics::ffi_manager();
    let mut payload = None;
    let result: Result<serde_json::Value, (&'static str, String)> = match request {
        Request::Diagnostics => diagnostics(context).await,
        Request::Capabilities => Ok(serde_json::json!({
            "protocol_version": 2,
            "operations": ["diagnostics","capabilities","resolution","screenshot","ui_tree","pointer_move","pointer_button","pointer_click","click","key","press_key","text"],
            "scene_layers": ["screen","top","system"],
            "screenshot": "composited_rgba8888",
            "input_acknowledgement": "queued",
            "visibility": "geometry_hidden_opacity_not_occlusion",
            "style_parts": ["main"],
            "per_app_profiling": false, "log_stream": false,
            "limits": {"response_bytes":MAX_RESPONSE_BYTES,"tree_elements":256,"tree_depth":24,"matrix_buttons_per_widget":64,"matrix_buttons_per_tree":128}
        })),
        Request::Click {
            role,
            name,
            text,
            button,
        } => graphics_manager
            .click_ui(
                role.as_deref(),
                name.as_deref(),
                text.as_deref(),
                button.as_deref(),
            )
            .await
            .map(|(x, y)| serde_json::json!({"x":x,"y":y,"status":"queued"}))
            .map_err(|code| {
                (
                    code,
                    "Control could not be uniquely and safely clicked.".into(),
                )
            }),
        Request::PointerClick { x, y } => match (i16::try_from(x), i16::try_from(y)) {
            (Ok(x), Ok(y)) => graphics_manager
                .inject_pointer_click(graphics::Point::new(x, y))
                .await
                .map(|()| serde_json::json!({"status":"queued"}))
                .map_err(input_error),
            _ => Err((
                "invalid_coordinates",
                "Coordinates are outside the display.".into(),
            )),
        },
        Request::Resolution => graphics_manager
            .get_resolution()
            .await
            .map(|point| serde_json::json!({"width": point.get_x(), "height": point.get_y()}))
            .map_err(|error| ("display_unavailable", error.to_string())),
        Request::Screenshot { path: _ } => match graphics_manager.capture_screen().await {
            Ok(snapshot) => {
                let metadata = ScreenshotMetadata {
                    width: snapshot.width,
                    height: snapshot.height,
                    format: ScreenshotPixelFormat::Rgba8888,
                    payload_bytes: snapshot.pixels_rgba.len(),
                };
                if debug_protocol::validate_screenshot_metadata(metadata).is_err() {
                    Err((
                        "screenshot_failed",
                        "Snapshot has invalid dimensions.".into(),
                    ))
                } else {
                    payload = Some(snapshot.pixels_rgba);
                    Ok(serde_json::json!({
                        "width": metadata.width,
                        "height": metadata.height,
                        "format": metadata.format,
                        "payload_bytes": metadata.payload_bytes,
                    }))
                }
            }
            Err(error) => Err(("screenshot_failed", error.to_string())),
        },
        Request::UiTree {
            max_elements,
            max_depth,
        } => match graphics::ffi_manager()
            .inspect_ui_tree(max_elements, max_depth)
            .await
        {
            Ok(snapshot) => Ok(inspection::encode(snapshot)),
            Err(error) => Err(("ui_tree_failed", error.to_string())),
        },
        Request::PointerMove { x, y } => {
            let point = match (i16::try_from(x), i16::try_from(y)) {
                (Ok(x), Ok(y)) => graphics::Point::new(x, y),
                _ => {
                    return DispatchResponse::json(error_response(
                        "invalid_coordinates",
                        "Coordinates are outside the display.",
                    ));
                }
            };
            match graphics_manager.inject_pointer_move(point).await {
                Ok(()) => Ok(serde_json::json!({"x": x, "y": y})),
                Err(graphics::Error::InvalidParameter) => Err((
                    "invalid_coordinates",
                    "Coordinates are outside the display.".into(),
                )),
                Err(error) => Err(input_error(error)),
            }
        }
        Request::PointerButton { state } => graphics_manager
            .inject_pointer_button(state_to_graphics(state))
            .await
            .map(|()| serde_json::json!({"state": state}))
            .map_err(input_error),
        Request::Key { key: name, state } => match parse_key(&name) {
            Some(key) => graphics_manager
                .inject_key(key, state_to_graphics(state))
                .await
                .map(|()| serde_json::json!({"key": name, "state": state}))
                .map_err(input_error),
            None => Err(("unsupported_key", "Key is not supported.".into())),
        },
        Request::PressKey { key: name } => match parse_key(&name) {
            Some(key) => graphics_manager
                .inject_key_press(key)
                .await
                .map(|()| serde_json::json!({"key": name, "status": "queued"}))
                .map_err(input_error),
            None => Err(("unsupported_key", "Key is not supported.".into())),
        },
        Request::Text { text } => match graphics_manager.inject_text_chunk(&text).await {
            Ok(count) => Ok(serde_json::json!({
                "status": "queued",
                "queued_bytes": count,
                "queued_events": text[..count].chars().count() * 2,
                "complete": count == text.len(),
                "unicode_window_events": if text[..count].is_ascii() { "not_applicable" } else { "unsupported" },
            })),
            Err(graphics::Error::InvalidParameter) => Err((
                "unsupported_character",
                "Control characters are not supported.".into(),
            )),
            Err(graphics::Error::AlreadyInUse) => Err((
                "queue_full",
                "The input queue is full; retry the unqueued text.".into(),
            )),
            Err(error) => Err(input_error(error)),
        },
    };

    let value = match result {
        Ok(value) => serde_json::json!({"ok": true, "result": value}),
        Err((code, message)) => error_response(code, &message),
    };
    DispatchResponse { value, payload }
}

fn input_error(error: graphics::Error) -> (&'static str, String) {
    match error {
        graphics::Error::AlreadyInUse => (
            "busy",
            "The input queue is full; retry after the UI processes pending input.".into(),
        ),
        error => ("input_failed", error.to_string()),
    }
}

fn parse_key(value: &str) -> Option<Key> {
    Some(match value {
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "escape" => Key::Escape,
        "delete" => Key::Delete,
        "backspace" => Key::Backspace,
        "enter" => Key::Enter,
        "next" => Key::Next,
        "previous" => Key::Previous,
        "home" => Key::Home,
        "end" => Key::End,
        value if value.len() == 1 && value.as_bytes()[0].is_ascii_graphic() => {
            Key::Character(value.as_bytes()[0])
        }
        _ => return None,
    })
}

fn state_to_graphics(state: InputState) -> State {
    match state {
        InputState::Pressed => State::Pressed,
        InputState::Released => State::Released,
    }
}

mod diagnostics;

use diagnostics::diagnostics;

fn error_response(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({"ok": false, "error": {"code": code, "message": message}})
}

fn code(value: u8) -> NonZeroUsize {
    NonZeroUsize::new(value as usize).unwrap()
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod window_setup_tests {
    extern crate std;

    use alloc::boxed::Box;
    use core::{
        ptr::NonNull,
        task::{Context, Poll},
        time::Duration,
    };
    use std::alloc::{GlobalAlloc, System};
    use std::sync::OnceLock;
    use xila::{
        file_system::{
            self, ControlCommand, ControlCommandIdentifier, DirectBaseOperations,
            DirectCharacterDevice, MountOperations, Size,
        },
        graphics::{self, GET_RESOLUTION, Point, SET_DRAWING_AREA, WAS_RESIZED},
        memory::{CapabilityFlags, Layout, ManagerTrait},
        shared::AnyByLayout,
        time::MonotonicClock,
    };

    use super::{TRANSPORT_PATH, TransportEnd, code, create_debug_window_with, serve_agent};
    use portable_atomic::AtomicBool;
    use xila::synchronization::Arc;

    struct TestMemoryManager;

    impl ManagerTrait for TestMemoryManager {
        unsafe fn allocate(&self, _: CapabilityFlags, layout: Layout) -> Option<NonNull<u8>> {
            NonNull::new(unsafe { System.alloc(layout) })
        }

        unsafe fn deallocate(&self, pointer: NonNull<u8>, layout: Layout) {
            unsafe { System.dealloc(pointer.as_ptr(), layout) };
        }

        fn get_used(&self) -> usize {
            0
        }

        fn get_free(&self) -> usize {
            isize::MAX as usize
        }
    }

    static TEST_MEMORY_MANAGER: TestMemoryManager = TestMemoryManager;

    #[global_allocator]
    #[unsafe(no_mangle)]
    pub static __XILA_MEMORY_MANAGER: xila::memory::Manager<'static> =
        xila::memory::Manager::new(&TEST_MEMORY_MANAGER);

    struct FixedClock;
    impl MonotonicClock for FixedClock {
        fn now(&self) -> Duration {
            Duration::ZERO
        }
    }

    struct HeadlessScreen;
    impl MountOperations for HeadlessScreen {}
    impl DirectCharacterDevice for HeadlessScreen {}
    impl DirectBaseOperations for HeadlessScreen {
        fn read(
            &self,
            _: &mut Context<'_>,
            _: &mut [u8],
            _: Size,
        ) -> Poll<file_system::Result<usize>> {
            Poll::Ready(Err(file_system::IoErrorKind::Unsupported))
        }

        fn write(
            &self,
            _: &mut Context<'_>,
            bytes: &[u8],
            _: Size,
        ) -> Poll<file_system::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }

        fn control(
            &self,
            _: &mut Context<'_>,
            command: ControlCommandIdentifier,
            _: &AnyByLayout,
            output: &mut AnyByLayout,
        ) -> Poll<file_system::Result<()>> {
            Poll::Ready(match command {
                GET_RESOLUTION::IDENTIFIER => GET_RESOLUTION::cast_output(output)
                    .map(|resolution| *resolution = Point::new(320, 240)),
                WAS_RESIZED::IDENTIFIER => {
                    WAS_RESIZED::cast_output(output).map(|resized| *resized = 0)
                }
                SET_DRAWING_AREA::IDENTIFIER => Ok(()),
                _ => Err(file_system::IoErrorKind::Unsupported),
            })
        }
    }

    fn headless_graphics_manager() -> &'static graphics::Manager {
        static MANAGER: OnceLock<&'static graphics::Manager> = OnceLock::new();
        MANAGER.get_or_init(|| {
            let time = Box::leak(Box::new(xila::time::Manager::with_clock(&FixedClock)));
            let manager: &'static graphics::Manager = Box::leak(Box::new(xila::task::block_on(
                graphics::initialize_with_input_devices(time, &HeadlessScreen, &[], 1024, false),
            )));
            graphics::set_ffi_manager(manager);
            manager
        })
    }

    /// A transport another opening already holds, like the background
    /// instance's.
    struct HeldTransport;
    impl MountOperations for HeldTransport {}
    impl DirectCharacterDevice for HeldTransport {}
    impl DirectBaseOperations for HeldTransport {
        fn open(&self) -> file_system::Result<()> {
            Err(file_system::IoErrorKind::ResourceBusy)
        }

        fn read(
            &self,
            _: &mut Context<'_>,
            _: &mut [u8],
            _: Size,
        ) -> Poll<file_system::Result<usize>> {
            Poll::Ready(Err(file_system::IoErrorKind::Unsupported))
        }

        fn write(
            &self,
            _: &mut Context<'_>,
            _: &[u8],
            _: Size,
        ) -> Poll<file_system::Result<usize>> {
            Poll::Ready(Err(file_system::IoErrorKind::Unsupported))
        }
    }

    #[xila::task::test(task_path = xila::task)]
    async fn a_transport_served_by_another_instance_is_not_an_error() {
        let standard = testing::initialize(false, false).await;
        let context = standard.context;
        let vfs = context.virtual_file_system;
        let task = context.task_manager.get_current_task_identifier().await;
        let stopping = || Arc::new(AtomicBool::new(false));

        // Without a transport device, the failure is still reported.
        assert!(serve_agent(vfs, context, task, stopping()).await.is_err());

        for directory in ["/devices/graphics", "/devices/graphics/debug"] {
            let _ = vfs.create_directory(task, &directory).await;
        }
        vfs.mount_character_device(task, TRANSPORT_PATH, HeldTransport)
            .await
            .unwrap();
        assert_eq!(
            serve_agent(vfs, context, task, stopping()).await,
            Ok(TransportEnd::ServedElsewhere)
        );
    }

    #[test]
    fn debug_window_setup_failure_deletes_window_without_deadlocking() {
        let manager = headless_graphics_manager();
        let baseline_windows = manager
            .with_lvgl_sync(|lvgl| manager.get_window_count(lvgl))
            .unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result =
                xila::task::block_on(create_debug_window_with(manager, |window, _guard| {
                    // SAFETY: the guard held by the caller owns the graphics
                    // lock and this closure runs inside its session scope.
                    let child = unsafe { graphics::lvgl::lv_obj_create(window.as_object()) };
                    assert!(!child.is_null());
                    Err(code(1))
                }));
            let _ = sender.send(result.err());
        });

        let error = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| {
                // A deadlocked worker keeps the graphics lock forever, so later
                // tests in this binary would block on it: exit instead of panicking.
                std::eprintln!(
                    "create_debug_window_with hung (likely deadlocked on the graphics lock)"
                );
                std::process::exit(1)
            });
        handle.join().unwrap();

        assert_eq!(error, Some(code(1)));
        assert!(
            manager.try_lock().is_some(),
            "graphics lock must be released after debug window setup error"
        );
        assert_eq!(
            manager
                .with_lvgl_sync(|lvgl| manager.get_window_count(lvgl))
                .unwrap(),
            baseline_windows
        );
    }
}

const WINDOW_ICON_COLOR: graphics::Color = graphics::Color::new(235, 87, 87);

#[cfg(test)]
#[test]
fn window_icon_matches_platform_shortcut() {
    // Keep this in sync with Platforms' tools/fs_image/shortcuts/debug.json.
    assert_eq!(WINDOW_ICON_COLOR, graphics::Color::new(235, 87, 87));
}
