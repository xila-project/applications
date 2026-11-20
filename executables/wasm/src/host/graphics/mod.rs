//! The Wasm transport of Core's graphics executor.
//!
//! Core owns the graphics service: the call table, the lock, each process's
//! object handles and the event queue. A guest reaches it through the
//! `xila_graphics_call` import, and the only thing this module adds is the
//! translation of guest addresses: the guest's linear memory is the transport,
//! with offsets as pointers and words as wide as the guest's address space.
pub mod wasmi;

use crate::host::translation::{GuestMemory, WasmUsize};
use xila::graphics::call::{self, Call, LinearMemory, Word};
pub use xila::graphics::call::{Error as GraphicsError, Function, State as GraphicsState};

/// One call as the guest passed it, in guest words.
pub struct GraphicsCall {
    pub function: i32,
    pub arguments: [WasmUsize; 7],
    pub count: i32,
    pub result: WasmUsize,
}

// A guest word must fit the host's: 64-bit guest memory only runs on 64-bit hosts.
const _: () = assert!(WasmUsize::BITS <= Word::BITS);

/// Run one guest call on the guest's graphics state.
pub fn dispatch(
    state: &mut GraphicsState,
    memory: &mut GuestMemory<'_>,
    call: GraphicsCall,
) -> Result<(), GraphicsError> {
    call::execute(
        state,
        &mut LinearMemory::new(memory.as_mut_slice(), WasmUsize::BITS),
        &Call {
            function: call.function,
            arguments: call.arguments.map(|word| word as Word),
            count: call.count,
            result: call.result as Word,
        },
    )
}
