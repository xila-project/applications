mod engine;
mod error;
mod executable;
pub mod execution;
#[cfg(feature = "graphics")]
pub mod graphics;
mod main;
mod registrable;
mod store;
pub mod translation;
mod wasi;

pub use executable::*;
pub use main::execute_with_control;
pub use store::GlobalStore;
pub use wasi::WasiContext;
pub use wasi::register::add_wasi_to_linker;

use core::sync::atomic::{AtomicBool, Ordering};

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request cooperative cancellation of the currently executing native WASM guest.
/// This is safe to call from a native signal handler.
pub fn request_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::Relaxed);
}

/// Clear a prior native shutdown request before starting a new guest.
pub fn clear_shutdown_request() {
    SHUTDOWN_REQUESTED.store(false, Ordering::Relaxed);
}

pub(crate) fn shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Relaxed)
}

/// A monotonic cancellation token owned by one execution, suitable for hot reload.
/// Clone the Arc before launching to retain a supervisor handle.
#[derive(Default)]
pub struct ExecutionControl(AtomicBool);
impl ExecutionControl {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn request_stop(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod control_tests {
    use super::ExecutionControl;
    #[test]
    fn cancellation_is_per_instance_and_monotonic() {
        let first = alloc::sync::Arc::new(ExecutionControl::new());
        let second = ExecutionControl::new();
        let handle = first.clone();
        handle.request_stop();
        assert!(first.is_stopped());
        assert!(!second.is_stopped());
        drop(handle);
        assert!(first.is_stopped());
    }
}
