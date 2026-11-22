//! A global allocator that records the live and peak heap, for tests that
//! measure what loading and running a guest costs. Each test binary that uses
//! it must hold a single test, since the counters are process wide.

use core::{
    alloc::GlobalAlloc,
    ptr::NonNull,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
};
use std::alloc::System;

use xila::memory::{CapabilityFlags, Layout, ManagerTrait};

pub struct TrackingMemoryManager {
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

    fn record_allocation(&self, size: usize) {
        let next = self.live_bytes.fetch_add(size as isize, Ordering::Relaxed) + size as isize;
        if next > 0 {
            self.peak_bytes.fetch_max(next as usize, Ordering::Relaxed);
        }
    }

    fn record_deallocation(&self, size: usize) {
        self.live_bytes.fetch_sub(size as isize, Ordering::Relaxed);
    }

    pub fn current_live_bytes(&self) -> usize {
        self.live_bytes.load(Ordering::Acquire).max(0) as usize
    }

    pub fn reset_peak_to_current(&self) -> usize {
        let current = self.current_live_bytes();
        self.peak_bytes.store(current, Ordering::Release);
        current
    }

    pub fn peak_bytes(&self) -> usize {
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
        self.record_allocation(layout.size());
        Some(non_null)
    }

    unsafe fn deallocate(&self, pointer: NonNull<u8>, layout: Layout) {
        self.record_deallocation(layout.size());
        unsafe { System.dealloc(pointer.as_ptr(), layout) };
    }

    fn get_used(&self) -> usize {
        self.current_live_bytes()
    }

    fn get_free(&self) -> usize {
        0
    }
}

pub static TRACKING_MEMORY_MANAGER: TrackingMemoryManager = TrackingMemoryManager::new();
xila::memory::instantiate_global_allocator!(&TRACKING_MEMORY_MANAGER);
