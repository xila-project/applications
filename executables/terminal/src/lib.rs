#![no_std]

extern crate alloc;

xila::internationalization::i18n!("locales");

mod device;
mod error;
mod executable;
mod terminal;

use alloc::boxed::Box;
pub use executable::*;

use alloc::{string::String, vec, vec::Vec};
use core::fmt::Write;
use core::{num::NonZeroUsize, time::Duration};
use error::*;
use xila::executable::Standard;
use xila::graphics;
use xila::log;
use xila::task::{self, TaskIdentifier};
use xila::virtual_file_system::ItemStatic;

use crate::{error::Result, terminal::Terminal};

pub const SHORTCUT: &str = r#"
{
    "name": "Terminal",
    "command": "/applications/executables/terminal",
    "arguments": [],
    "terminal": false,
    "icon_string": ">_",
    "icon_color": [0, 0, 0]
}"#;

/// Device node of one terminal window.
///
/// `/devices` is writable by every user, unlike `/devices/standard_io`, which
/// only root may change. One node per task also keeps terminals independent.
fn device_path(task: TaskIdentifier) -> String {
    alloc::format!("/devices/terminal_{task}")
}

async fn mount_and_open(
    task: TaskIdentifier,
    terminal: &'static Terminal,
    standard: &Standard<'static>,
) -> Result<Standard<'static>> {
    let virtual_file_system = &standard.context.virtual_file_system;
    let path = device_path(task);
    let path = path.as_str();

    // A stale node may remain if an earlier terminal reused this identifier.
    let _ = virtual_file_system.remove(task, &path).await;

    virtual_file_system
        .mount_static(task, &path, ItemStatic::CharacterDevice(terminal))
        .await?;

    let standard = Standard::open(
        &path,
        &path,
        &path,
        task,
        virtual_file_system,
        standard.context,
    )
    .await?;

    Ok(standard)
}

async fn inner_main(task: TaskIdentifier, standard: &Standard<'static>) -> Result<()> {
    let terminal = Terminal::new(graphics::ffi_manager()).await?;

    let terminal = Box::leak(Box::new(terminal));
    let virtual_file_system = standard.context.virtual_file_system;

    let standard = mount_and_open(task, terminal, standard).await?;

    xila::executable::execute(
        standard.context,
        "/applications/executables/command_line_shell",
        vec![],
        standard,
        None,
    )
    .await?;

    while terminal.handle_events().await? {
        task::Manager::sleep(Duration::from_millis(20)).await;
    }

    let _ = virtual_file_system
        .remove(task, &device_path(task).as_str())
        .await;

    Ok(())
}

pub async fn main(
    mut standard: Standard<'static>,
    _: Vec<String>,
) -> core::result::Result<(), NonZeroUsize> {
    let task_manager = &standard.context.task_manager;
    let task = task_manager.get_current_task_identifier().await;

    if let Err(error) = inner_main(task, &standard).await {
        let _ = writeln!(standard.error(), "{}", error);
        log::error!("Terminal executable error: {}", error);
        return Err(error.into());
    }

    Ok(())
}
