use crate::main;
use alloc::boxed::Box;
use alloc::string::String;
use xila::executable::ExecutableTrait;
use xila::task::TaskIdentifier;
use xila::virtual_file_system::VirtualFileSystem;

pub struct TerminalExecutable;

impl TerminalExecutable {
    /// Construct the executable without mutating the filesystem.
    /// Shell shortcut discovery is handled by the graphical Shell.
    pub async fn new(
        _virtual_file_system: &VirtualFileSystem,
        _task: TaskIdentifier,
    ) -> Result<Self, String> {
        Ok(Self)
    }
}

impl ExecutableTrait for TerminalExecutable {
    fn main(
        standard: xila::executable::Standard<'static>,
        arguments: alloc::vec::Vec<alloc::string::String>,
    ) -> xila::executable::MainFuture {
        Box::pin(async move { main(standard, arguments).await })
    }
}
