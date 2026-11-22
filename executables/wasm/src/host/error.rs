use core::fmt::Display;

use xila::virtual_file_system;

pub type Result<T> = core::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Wasm(wasmi::Error),
    Linker(wasmi::errors::LinkerError),
    Runtime(i32),
    Interrupted,
    FileSystem(virtual_file_system::IoErrorKind),
    Task(xila::task::Error),
    InvalidPath,
    NotAWasmFile,
    /// The executable was refused before compilation.
    Integrity(super::integrity::IntegrityError),
}

impl From<wasmi::Error> for Error {
    fn from(error: wasmi::Error) -> Self {
        Self::Wasm(error)
    }
}

impl From<wasmi::errors::LinkerError> for Error {
    fn from(error: wasmi::errors::LinkerError) -> Self {
        Self::Linker(error)
    }
}

impl From<virtual_file_system::IoErrorKind> for Error {
    fn from(error: virtual_file_system::IoErrorKind) -> Self {
        Self::FileSystem(error)
    }
}

impl From<xila::task::Error> for Error {
    fn from(error: xila::task::Error) -> Self {
        Self::Task(error)
    }
}

impl From<super::integrity::IntegrityError> for Error {
    fn from(error: super::integrity::IntegrityError) -> Self {
        Self::Integrity(error)
    }
}

impl Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("An error occurred")
    }
}

impl core::error::Error for Error {}
