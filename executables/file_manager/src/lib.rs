#![no_std]

extern crate alloc;

xila::internationalization::track_locales!();

mod error;
mod file_manager;

use alloc::boxed::Box;
use alloc::string::String;
use core::num::NonZeroUsize;
use xila::executable::{self, ExecutableTrait, Standard};
use xila::task::TaskIdentifier;
use xila::virtual_file_system::VirtualFileSystem;

pub use error::*;
pub use file_manager::*;

pub struct FileManagerExecutable;

impl FileManagerExecutable {
    /// Kept for native callers; launch metadata belongs to the Shell.
    pub async fn new(
        _virtual_file_system: &VirtualFileSystem,
        _task: TaskIdentifier,
    ) -> core::result::Result<Self, String> {
        Ok(Self)
    }
}

impl ExecutableTrait for FileManagerExecutable {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
        Box::pin(async move { main(standard, arguments).await })
    }
}

pub async fn main(
    standard: Standard<'static>,
    _: Vec<String>,
) -> core::result::Result<(), NonZeroUsize> {
    let mut file_manager = FileManager::new(standard.context)
        .await
        .map_err(|_| NonZeroUsize::new(1).unwrap())?;

    while file_manager.handle_events().await {}

    Ok(())
}
