#![allow(clippy::disallowed_types, reason = "host-only integration test")]

//! Heap use of one package download and of one redirect chain, measured against
//! a scripted HTTPS client device.

use core::{
    alloc::GlobalAlloc,
    ptr::NonNull,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
    task::{Context, Poll},
};
use std::{alloc::System, collections::VecDeque, sync::Mutex};

extern crate alloc;

// The downloader is private to the crate, so the test compiles its source directly.
#[path = "../src/http.rs"]
#[allow(dead_code, reason = "only part of the downloader is exercised")]
mod http;

use http::Sink;
use xila::{
    file_system::{
        DirectBaseOperations, DirectCharacterDevice, IoErrorKind, MountOperations, Path, Size,
    },
    memory::{CapabilityFlags, Layout, ManagerTrait},
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

/// The HTTPS client device: every request written starts the next scripted
/// response, published as its head on its own followed by body chunks.
struct ScriptedHttpsClient {
    state: Mutex<ScriptedState>,
}

struct ScriptedState {
    responses: VecDeque<(&'static [u8], usize)>,
    head: Option<&'static [u8]>,
    body_remaining: usize,
}

impl ScriptedHttpsClient {
    /// Heads are leaked so that serving them never frees memory inside a measurement.
    fn new(responses: Vec<(Vec<u8>, usize)>) -> Self {
        let responses: Vec<(&'static [u8], usize)> = responses
            .into_iter()
            .map(|(head, body_length)| (&*Box::leak(head.into_boxed_slice()), body_length))
            .collect();
        Self {
            state: Mutex::new(ScriptedState {
                responses: responses.into(),
                head: None,
                body_remaining: 0,
            }),
        }
    }
}

impl MountOperations for ScriptedHttpsClient {}
impl DirectCharacterDevice for ScriptedHttpsClient {}

impl DirectBaseOperations for ScriptedHttpsClient {
    fn read(
        &self,
        _: &mut Context<'_>,
        buffer: &mut [u8],
        _: Size,
    ) -> Poll<xila::file_system::Result<usize>> {
        let mut state = self.state.lock().unwrap();
        // The head is published on its own, then the body in buffer-sized reads.
        if let Some(head) = state.head.take() {
            let count = buffer.len().min(head.len());
            buffer[..count].copy_from_slice(&head[..count]);
            assert_eq!(
                count,
                head.len(),
                "the head buffer must hold the whole head"
            );
            return Poll::Ready(Ok(count));
        }
        let count = buffer.len().min(state.body_remaining);
        buffer[..count].fill(0x5a);
        state.body_remaining -= count;
        Poll::Ready(Ok(count))
    }

    fn write(
        &self,
        _: &mut Context<'_>,
        buffer: &[u8],
        _: Size,
    ) -> Poll<xila::file_system::Result<usize>> {
        let mut state = self.state.lock().unwrap();
        let Some((head, body_length)) = state.responses.pop_front() else {
            return Poll::Ready(Err(IoErrorKind::Unsupported));
        };
        state.head = Some(head);
        state.body_remaining = body_length;
        Poll::Ready(Ok(buffer.len()))
    }
}

/// A response head padded to `head_length` bytes with a filler header.
fn head(status_line: &str, extra: &str, head_length: usize) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status_line}\r\n{extra}X-Padding: ").into_bytes();
    let tail = b"\r\n\r\n";
    head.resize(head_length - tail.len(), b'a');
    head.extend_from_slice(tail);
    head
}

struct CountingSink {
    received: usize,
}

impl Sink for CountingSink {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.received += bytes.len();
        Ok(())
    }
}

/// The downloader's head buffer (`HEAD_BUFFER`).
const HEAD_BUFFER_LENGTH: usize = 8192;
const BODY_LENGTH: usize = 256 * 1024;
const SUCCESS_HEAD_LENGTH: usize = 1024;
/// GitHub release redirects need about 5.5 KB of head.
const REDIRECT_HEAD_LENGTH: usize = 5632;

#[xila::task::test(task_path = xila::task)]
async fn a_download_and_a_redirect_chain_hold_a_bounded_head_and_body_buffer() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;

    let redirect = |location: &str| {
        (
            head(
                "302 Found",
                &format!("Location: https://objects.example/{location}\r\n"),
                REDIRECT_HEAD_LENGTH,
            ),
            0,
        )
    };
    let success = || (head("200 OK", "", SUCCESS_HEAD_LENGTH), BODY_LENGTH);

    // One device serves every scenario: each request written starts the next response.
    let scenarios = [
        "one package download",
        "redirect chain (two redirects, then the package)",
    ];
    let responses = vec![success(), redirect("one"), redirect("two"), success()];

    context
        .virtual_file_system
        .create_directory(task, &"/devices/network")
        .await
        .ok();
    context
        .virtual_file_system
        .mount_character_device(
            task,
            Path::from_str("/devices/network/https_client"),
            ScriptedHttpsClient::new(responses),
        )
        .await
        .unwrap();

    for label in scenarios {
        let mut sink = CountingSink { received: 0 };
        let baseline = TRACKING_MEMORY_MANAGER.reset_peak_to_current();
        http::get(
            context.virtual_file_system,
            task,
            "https://github.com/xila-project/example/releases/download/v1/package.xpk",
            &mut sink,
        )
        .await
        .unwrap();
        let peak = TRACKING_MEMORY_MANAGER
            .peak_bytes()
            .saturating_sub(baseline);
        let after = TRACKING_MEMORY_MANAGER
            .current_live_bytes()
            .saturating_sub(baseline);
        assert_eq!(sink.received, BODY_LENGTH, "{label}: streamed body");
        println!("download peak heap | {label}: {peak} B (live after: {after} B)");
        // The 8 KiB head buffer is the only large allocation: the body chunks reuse it.
        assert!(
            peak < HEAD_BUFFER_LENGTH + HEAD_BUFFER_LENGTH / 8,
            "{label}: peak {peak} B exceeds the head buffer plus bookkeeping"
        );
        assert_eq!(after, 0, "{label}: heap retained after the download");
    }
}
