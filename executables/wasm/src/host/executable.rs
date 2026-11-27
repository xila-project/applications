use alloc::boxed::Box;
use xila::executable::ExecutableTrait;

use crate::host::main::execute;

/// Default extra bytes for guests without a declared memory maximum.
pub const DEFAULT_MEMORY_GROWTH_ALLOWANCE: usize = 64 * 1024;

pub struct WasmExecutable;

impl WasmExecutable {
    /// Create a runner with a board-specific allowance in bytes.
    pub const fn with_memory_growth_allowance<const BYTES: usize>() -> impl ExecutableTrait {
        ConfiguredWasmExecutable::<BYTES>
    }
}

impl ExecutableTrait for WasmExecutable {
    fn main(
        standard: xila::executable::Standard<'static>,
        arguments: alloc::vec::Vec<alloc::string::String>,
    ) -> xila::executable::MainFuture {
        ConfiguredWasmExecutable::<DEFAULT_MEMORY_GROWTH_ALLOWANCE>::main(standard, arguments)
    }
}

struct ConfiguredWasmExecutable<const BYTES: usize>;

impl<const BYTES: usize> ExecutableTrait for ConfiguredWasmExecutable<BYTES> {
    fn main(
        standard: xila::executable::Standard<'static>,
        arguments: alloc::vec::Vec<alloc::string::String>,
    ) -> xila::executable::MainFuture {
        Box::pin(execute(standard, arguments, Default::default(), BYTES))
    }
}
