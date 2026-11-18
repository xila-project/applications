use core::fmt::Display;

use crate::host::wasi::WasiContext;

#[allow(
    clippy::disallowed_types,
    reason = "xila::synchronization::Arc resolves to alloc::sync::Arc until core#650 merges"
)]
pub struct GlobalStore {
    pub control: xila::synchronization::Arc<super::ExecutionControl>,
    pub context: &'static xila::executable::ExecutableContext,
    pub wasi: WasiContext,
    pub pending: Option<super::execution::PendingCall>,
    #[cfg(feature = "graphics")]
    pub graphics: super::graphics::GraphicsState,
}

impl Display for GlobalStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "GlobalStore")
    }
}
