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
#[path = "support/graphics.rs"]
mod support;

use core::{
    ffi::CStr,
    future::{Future, poll_fn},
    task::Poll,
    time::Duration,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use xila::{
    file_system::{
        BaseOperations, CharacterDevice, Context, ControlCommandIdentifier, Error, MountOperations,
        Result, Size,
    },
    graphics::{Manager, lvgl},
};

/// Playback device that reports every third write and every third control as
/// busy, as the real mixer does when its thread holds the nonblocking queue lock.
struct ContendedPlayback {
    device: audio::AudioStreamDevice,
    writes: AtomicUsize,
    controls: AtomicUsize,
    busy: &'static AtomicUsize,
}

impl ContendedPlayback {
    fn contended(&self, calls: &AtomicUsize) -> bool {
        if calls.fetch_add(1, Ordering::Relaxed).is_multiple_of(3) {
            self.busy.fetch_add(1, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

impl BaseOperations for ContendedPlayback {
    fn open(&self, context: &mut Context) -> Result<()> {
        self.device.open(context)
    }

    fn close(&self, context: &mut Context) -> Result<()> {
        self.device.close(context)
    }

    fn read(
        &self,
        context: &mut Context,
        cx: &mut core::task::Context<'_>,
        buffer: &mut [u8],
        position: Size,
    ) -> Poll<Result<usize>> {
        self.device.read(context, cx, buffer, position)
    }

    fn write(
        &self,
        context: &mut Context,
        cx: &mut core::task::Context<'_>,
        buffer: &[u8],
        position: Size,
    ) -> Poll<Result<usize>> {
        if self.contended(&self.writes) {
            return Poll::Ready(Err(Error::RessourceBusy));
        }
        self.device.write(context, cx, buffer, position)
    }

    fn flush(&self, context: &mut Context, cx: &mut core::task::Context<'_>) -> Poll<Result<()>> {
        self.device.flush(context, cx)
    }

    fn control(
        &self,
        context: &mut Context,
        cx: &mut core::task::Context<'_>,
        command: ControlCommandIdentifier,
        input: &xila::shared::AnyByLayout,
        output: &mut xila::shared::AnyByLayout,
    ) -> Poll<Result<()>> {
        if self.contended(&self.controls) {
            return Poll::Ready(Err(Error::RessourceBusy));
        }
        self.device.control(context, cx, command, input, output)
    }

    fn clone_context(&self, context: &Context) -> Result<Context> {
        self.device.clone_context(context)
    }
}

impl MountOperations for ContendedPlayback {}
impl CharacterDevice for ContendedPlayback {}

unsafe fn label(root: *mut lvgl::lv_obj_t, text: &str) -> *mut lvgl::lv_obj_t {
    unsafe {
        if lvgl::lv_obj_has_class(root, &lvgl::lv_label_class)
            && CStr::from_ptr(lvgl::lv_label_get_text(root))
                .to_bytes()
                .starts_with(text.as_bytes())
        {
            return root;
        }
        for index in 0..lvgl::lv_obj_get_child_count(root) {
            let found = label(lvgl::lv_obj_get_child(root, index as i32), text);
            if !found.is_null() {
                return found;
            }
        }
        core::ptr::null_mut()
    }
}

async fn wait_label(manager: &Manager, text: &str) {
    for _ in 0..400 {
        if manager.get_window_count().await.unwrap() != 0 {
            let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
            let _lock = manager.synchronous_lock();
            if unsafe { !label(root, text).is_null() } {
                return;
            }
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Synth did not display {text:?}");
}

/// Selects a value on the first widget of `class` and reports it to the guest.
async fn change(manager: &Manager, class: *const lvgl::lv_obj_class_t, value: i32) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let widget = find_class(root, class);
        assert!(!widget.is_null(), "missing Synth widget");
        if class == &raw const lvgl::lv_dropdown_class {
            lvgl::lv_dropdown_set_selected(widget, value as u32);
        } else {
            lvgl::lv_slider_set_value(widget, value, false);
        }
        lvgl::lv_obj_send_event(
            widget,
            lvgl::lv_event_code_t_LV_EVENT_VALUE_CHANGED,
            core::ptr::null_mut(),
        );
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

async fn event(manager: &Manager, text: &str, code: u32) {
    let root = manager.get_window_identifier(0).await.unwrap() as *mut lvgl::lv_obj_t;
    let _lock = manager.synchronous_lock();
    unsafe {
        let found = label(root, text);
        assert!(!found.is_null(), "missing Synth control {text:?}");
        lvgl::lv_obj_send_event(lvgl::lv_obj_get_parent(found), code, core::ptr::null_mut());
    }
}

#[xila::task::test(task_path=xila::task)]
async fn real_synth_guest_recovers_from_silent_mode_plays_releases_and_closes() {
    let mut standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let context = support::context(&standard, Some(manager));
    standard.context = context;
    let task = context.task_manager.get_current_task_identifier().await;
    let binary = applications::build("xila-synth").unwrap();
    testing::load_file(
        context.virtual_file_system,
        binary,
        "/applications/executables/synth",
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
    let handle = xila::executable::execute(
        context,
        "/applications/executables/wasm",
        vec!["/applications/executables/synth".into()],
        standard,
        None,
    )
    .await
    .unwrap();
    wait_label(manager, "Silent mode").await;
    // A real pointer press still provides feedback without an audio device.
    event(manager, "C", lvgl::lv_event_code_t_LV_EVENT_PRESSED).await;
    wait_label(manager, "Playing C4").await;
    event(manager, "C", lvgl::lv_event_code_t_LV_EVENT_PRESS_LOST).await;
    wait_label(manager, "Hold a key").await;

    let system: &'static audio::AudioSystem =
        Box::leak(Box::new(audio::AudioSystem::new().unwrap()));
    // SAFETY: this leaked system never moves and outlives all device streams.
    let system = unsafe { core::pin::Pin::new_unchecked(system) };
    let mut output = system.open_output().unwrap();
    let busy: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
    let playback = ContendedPlayback {
        device: audio::AudioStreamDevice::new(system),
        writes: AtomicUsize::new(0),
        controls: AtomicUsize::new(0),
        busy,
    };
    context
        .virtual_file_system
        .mount_character_device(task, "/devices/audio/playback", playback)
        .await
        .unwrap();
    event(
        manager,
        "Retry audio",
        lvgl::lv_event_code_t_LV_EVENT_CLICKED,
    )
    .await;
    wait_label(manager, "Audio ready").await;
    change(manager, &raw const lvgl::lv_dropdown_class, 1).await;
    wait_label(manager, "Octave 4 | Square").await;
    event(manager, "Oct +", lvgl::lv_event_code_t_LV_EVENT_CLICKED).await;
    wait_label(manager, "Octave 5 | Square").await;
    event(manager, "C", lvgl::lv_event_code_t_LV_EVENT_PRESSED).await;
    wait_label(manager, "Playing C5").await;
    let mut heard = false;
    let mut bytes = [0u8; 1024];
    for _ in 0..100 {
        system.mix_next_block().unwrap();
        let count = output.with_consumer(|consumer| consumer.read(&mut bytes));
        heard |= bytes[..count].iter().any(|byte| *byte != 0);
        xila::task::sleep(Duration::from_millis(3)).await;
    }
    assert!(heard, "held note must reach the real VFS/PCM mixer");
    // Busy writes and controls are retryable (EAGAIN), not fatal (EIO): the
    // guest must keep its stream instead of falling back to silent mode.
    assert!(busy.load(Ordering::Relaxed) > 0);
    wait_label(manager, "Audio ready").await;
    event(manager, "C", lvgl::lv_event_code_t_LV_EVENT_RELEASED).await;
    for _ in 0..150 {
        system.mix_next_block().unwrap();
        output.with_consumer(|consumer| consumer.read(&mut bytes));
        xila::task::sleep(Duration::from_millis(3)).await;
    }
    assert!(
        bytes.iter().all(|byte| *byte == 0),
        "released note must become silent"
    );
    change(manager, &raw const lvgl::lv_slider_class, 60).await;
    wait_label(manager, "Octave 5 | Square | Volume 60%").await;
    manager
        .send_window_close_request(manager.get_window_identifier(0).await.unwrap())
        .await
        .unwrap();
    let mut join = core::pin::pin!(handle.join());
    for _ in 0..400 {
        if let Poll::Ready(status) = poll_fn(|cx| Poll::Ready(join.as_mut().poll(cx))).await {
            assert_eq!(status, 0);
            assert_eq!(manager.get_window_count().await.unwrap(), 0);
            // Closing the guest releases its stream reservation.
            let streams: Vec<_> = (0..4).map(|_| system.open_stream().unwrap()).collect();
            drop(streams);
            return;
        }
        xila::task::sleep(Duration::from_millis(5)).await;
    }
    panic!("Synth did not exit on close request");
}
