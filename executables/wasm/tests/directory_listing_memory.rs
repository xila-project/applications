#![cfg(feature = "host")]
#![allow(clippy::disallowed_types, reason = "host-only integration test")]

//! Heap use of the guest directory listing call (`fd_readdir`) for a large
//! directory read through a small guest buffer.

use core::{
    alloc::GlobalAlloc,
    ptr::NonNull,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};
use std::alloc::System;

use xila::{
    memory::{CapabilityFlags, Layout, ManagerTrait},
    synchronization::Arc,
    virtual_file_system::File,
};

struct TrackingMemoryManager {
    live_bytes: AtomicIsize,
    peak_bytes: AtomicUsize,
}

impl TrackingMemoryManager {
    const fn new() -> Self {
        Self {
            live_bytes: AtomicIsize::new(0),
            peak_bytes: AtomicUsize::new(0),
        }
    }

    fn current_live_bytes(&self) -> usize {
        self.live_bytes.load(Ordering::Acquire).max(0) as usize
    }

    fn reset_peak_to_current(&self) -> usize {
        let current = self.current_live_bytes();
        self.peak_bytes.store(current, Ordering::Release);
        current
    }

    fn peak_bytes(&self) -> usize {
        self.peak_bytes.load(Ordering::Acquire)
    }
}

unsafe impl Send for TrackingMemoryManager {}
unsafe impl Sync for TrackingMemoryManager {}

impl ManagerTrait for TrackingMemoryManager {
    unsafe fn allocate(
        &self,
        _capabilities: CapabilityFlags,
        layout: Layout,
    ) -> Option<NonNull<u8>> {
        let pointer = unsafe { System.alloc(layout) };
        let non_null = NonNull::new(pointer)?;
        let size = layout.size() as isize;
        let next = self.live_bytes.fetch_add(size, Ordering::Relaxed) + size;
        if next > 0 {
            self.peak_bytes.fetch_max(next as usize, Ordering::Relaxed);
        }
        Some(non_null)
    }

    unsafe fn deallocate(&self, pointer: NonNull<u8>, layout: Layout) {
        self.live_bytes
            .fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(pointer.as_ptr(), layout) };
    }

    fn get_used(&self) -> usize {
        self.current_live_bytes()
    }

    fn get_free(&self) -> usize {
        0
    }
}

static TRACKING_MEMORY_MANAGER: TrackingMemoryManager = TrackingMemoryManager::new();
xila::memory::instantiate_global_allocator!(&TRACKING_MEMORY_MANAGER);

const ENTRY_COUNT: usize = 300;
const BUFFER_LENGTH: usize = 64;
/// Every record is a 24-byte header followed by the name.
const NAME_LENGTH: usize = 40;
const RECORD_LENGTH: usize = 24 + NAME_LENGTH;
const DIRECTORY_LENGTH: usize = ENTRY_COUNT + 2;

/// A guest that opens `/bulk`, requests one buffer of directory records at
/// `cookie` and exits with the byte count written, so the host reports the
/// count back as the exit status. `cookie` equal to `u64::MAX` is the control:
/// it returns before touching the directory.
fn directory_guest(cookie: u64) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
            (import "wasi_snapshot_preview1" "path_open"
                (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
            (import "wasi_snapshot_preview1" "fd_readdir"
                (func $readdir (param i32 i32 i32 i64 i32) (result i32)))
            (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
            (memory (export "memory") 1)
            (data (i32.const 16) "bulk")
            (func (export "_start")
                ;; path_open(root, dirflags, "bulk", O_DIRECTORY, all rights, 0, 0, &fd)
                (if (call $open (i32.const 3) (i32.const 0) (i32.const 16) (i32.const 4)
                        (i32.const 2) (i64.const -1) (i64.const -1) (i32.const 0)
                        (i32.const 32))
                    (then (call $exit (i32.const 200))))
                (if (call $readdir (i32.load (i32.const 32)) (i32.const 1024)
                        (i32.const {BUFFER_LENGTH}) (i64.const {cookie}) (i32.const 40))
                    (then (call $exit (i32.const 201))))
                (call $exit (i32.load (i32.const 40)))))"#
    ))
    .unwrap()
}

#[xila::task::test(task_path = xila::task)]
async fn directory_listing_uses_space_bounded_by_the_guest_buffer() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;

    context
        .virtual_file_system
        .create_directory(task, &"/bulk")
        .await
        .unwrap();
    for index in 0..ENTRY_COUNT {
        let name = format!("{index:0>width$}", width = NAME_LENGTH);
        File::write_to_path(
            context.virtual_file_system,
            task,
            format!("/bulk/{name}").as_str(),
            b"",
        )
        .await
        .unwrap_or_else(|error| panic!("creating entry {index}: {error:?}"));
    }

    let path = "/directory-listing-guest.wasm";
    let mut peaks = Vec::new();
    for (label, cookie, expected_written) in [
        ("control (no listing)", u64::MAX, 0),
        ("first buffer (\".\" and \"..\")", 0, BUFFER_LENGTH),
        (
            "buffer near the end",
            (DIRECTORY_LENGTH - 3) as u64,
            RECORD_LENGTH,
        ),
        ("empty at the end", DIRECTORY_LENGTH as u64, 0),
    ] {
        File::write_to_path(
            context.virtual_file_system,
            task,
            path,
            &directory_guest(cookie),
        )
        .await
        .unwrap();

        let baseline = TRACKING_MEMORY_MANAGER.reset_peak_to_current();
        let result = wasm::execute_with_control(
            standard.duplicate().await.unwrap(),
            vec![path.to_string()],
            Arc::new(wasm::ExecutionControl::new()),
        )
        .await;
        let peak = TRACKING_MEMORY_MANAGER
            .peak_bytes()
            .saturating_sub(baseline);
        let written = match result {
            Ok(()) => 0,
            Err(code) => code.get(),
        };
        assert_eq!(written, expected_written, "{label}: bytes written");
        peaks.push((label, peak));
    }

    let control = peaks[0].1;
    for (label, peak) in &peaks {
        println!(
            "directory listing peak heap | {label}: {peak} B ({} B above control)",
            peak.saturating_sub(control)
        );
    }

    // The listing of 500 entries must not cost heap in proportion to the
    // directory: only the records that fit the guest buffer may be held.
    let directory_bytes = ENTRY_COUNT * RECORD_LENGTH;
    for (label, peak) in &peaks[1..] {
        assert!(
            peak.saturating_sub(control) < directory_bytes / 4,
            "{label}: peak {peak} B is {} B above the control for a {directory_bytes}-byte directory",
            peak.saturating_sub(control)
        );
    }
}
