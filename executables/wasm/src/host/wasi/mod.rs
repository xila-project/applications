#[cfg(all(test, feature = "memory_32"))]
mod conformance;
mod context;
mod descriptor;
mod directory;
mod environment;
mod error;
mod file;
mod offset;
mod path;
mod poll;
pub mod poll_memory;
#[cfg(test)]
mod poll_tests;
mod process;
mod random;
pub mod register;
mod rights;
mod scheduling;
mod socket;
mod time;
pub(crate) mod types;

pub use context::*;
pub use error::Error;

// Kept out of poll_memory.rs, which the translation test crate also compiles
// without the WASI error type.
impl From<poll_memory::PollError> for i32 {
    fn from(error: poll_memory::PollError) -> Self {
        use poll_memory::PollError;

        match error {
            PollError::Fault => Error::Fault,
            PollError::Invalid => Error::Inval,
            PollError::Io => Error::Io,
            PollError::NotSupported => Error::Notsup,
            PollError::Overflow => Error::Overflow,
        }
        .into()
    }
}
