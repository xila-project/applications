//! `poweroff` and `reboot`: privileged lifecycle requests.
//!
//! The lifecycle subsystem checks the shell user's privilege and the power
//! device's capabilities, then the platform stops every task (this shell
//! included), commits storage and performs the action.

use super::CommandContext;
use xila::lifecycle::{self, Action};

#[derive(getargs_derive::GetArgs)]
#[command(name = "poweroff", about = "Shut Xila down and power off.")]
pub(super) struct PoweroffArguments {}

#[derive(getargs_derive::GetArgs)]
#[command(name = "reboot", about = "Shut Xila down and restart.")]
pub(super) struct RebootArguments {}

pub async fn execute<'a, I: Iterator<Item = &'a str>>(
    context: &mut impl CommandContext,
    options: &mut getargs::Options<&'a str, I>,
    action: Action,
) -> crate::Result<(), crate::Error<'a>> {
    match action {
        Action::Restart => {
            RebootArguments::parse(options)?;
        }
        _ => {
            PoweroffArguments::parse(options)?;
        }
    }
    lifecycle::request(
        context.executable_context().virtual_file_system,
        context.task_id(),
        action,
        0,
    )
    .await
    .map_err(crate::Error::Lifecycle)?;
    // Leave now; the supervisor stops the remaining tasks.
    context.stop();
    Ok(())
}
