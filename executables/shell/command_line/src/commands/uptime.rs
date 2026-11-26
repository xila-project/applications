//! `uptime`: show how long the system has been up, from the monotonic clock.
//! The real-time clock never supplies elapsed time.

use getargs_derive::GetArgs;
use xila::file_system::Path;

use super::{CommandContext, UserCommand};
use crate::{Error, Result};

pub struct UptimeCommand;

#[derive(GetArgs)]
#[command(name = "uptime", about = "Show how long the system has been up.")]
pub(super) struct UptimeArguments {}

impl UserCommand for UptimeCommand {
    async fn execute<'a, I, C>(
        &self,
        context: &mut C,
        options: &mut getargs::Options<&'a str, I>,
        _paths: &[&Path],
    ) -> Result<(), Error<'a>>
    where
        I: Iterator<Item = &'a str>,
        C: CommandContext,
    {
        let _ = UptimeArguments::parse(options)?;
        let uptime = context.executable_context().time_manager.uptime();
        show_uptime(context, uptime).await
    }
}

async fn show_uptime<C: CommandContext>(
    context: &mut C,
    uptime: xila::time::Duration,
) -> Result<()> {
    let seconds = uptime.as_secs();
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if days > 0 {
        context
            .write_out_fmt(format_args!(
                "up {days} days, {hours:02}:{minutes:02}:{seconds:02}\n"
            ))
            .await?;
    } else {
        context
            .write_out_fmt(format_args!("up {hours:02}:{minutes:02}:{seconds:02}\n"))
            .await?;
    }
    Ok(())
}
