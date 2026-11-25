//! `real_time_clock`: the `hwclock`-like command over the
//! `/devices/time/real_time_clock` device node, through its control commands.
//!
//! Subcommands: `show` prints the UTC time the clock holds; `system_to_clock`
//! sets the clock from the system UTC time; `clock_to_system` sets the system
//! UTC time from the clock. The two set subcommands are root only: the device
//! node is mounted with owner-write permissions, so a write-mode open fails
//! for other users, and `clock_to_system` checks the task's user itself.

use core::time::Duration;

use device::real_time_clock::{
    ABI_VERSION, GET_TIME, NOT_VALID, RealTimeClockTimeAbi, SET_TIME, VALID,
};
use getargs_derive::GetArgs;
use xila::file_system::{AccessFlags, Path};
use xila::time::{ClockSource, Manager, RealtimeSample, Timestamp};
use xila::virtual_file_system::File;

use super::{CommandContext, UserCommand, civil_time};
use crate::{Error, Result};

pub struct RealTimeClockCommand;

#[derive(GetArgs)]
#[command(
    name = "real_time_clock",
    about = "Show or set the real-time clock (RTC) that stores UTC across resets: show | system_to_clock | clock_to_system (the two set subcommands are root only and need a root login: sudo runs external commands only)."
)]
pub(super) struct RealTimeClockArguments<'a> {
    /// The subcommand: `show` prints the UTC time the real-time clock holds;
    /// `system_to_clock` sets the clock from the system UTC time (root only);
    /// `clock_to_system` sets the system UTC time from the clock (root only).
    command: &'a str,
}

const SHOW: &str = "show";
const SYSTEM_TO_CLOCK: &str = "system_to_clock";
const CLOCK_TO_SYSTEM: &str = "clock_to_system";

impl UserCommand for RealTimeClockCommand {
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
        let arguments = RealTimeClockArguments::parse(options)?;
        match arguments.command {
            SHOW => show(context).await,
            SYSTEM_TO_CLOCK => system_to_clock(context).await,
            CLOCK_TO_SYSTEM => clock_to_system(context).await,
            _ => Err(Error::InvalidSubcommand("real_time_clock")),
        }
    }
}

/// Opens the real-time clock device node. A read-write open is reserved to
/// root by the node's permissions.
async fn open_device<C: CommandContext>(context: &C, access: AccessFlags) -> Result<File<'static>> {
    let executable_context = context.executable_context();
    let task = executable_context
        .task_manager
        .get_current_task_identifier()
        .await;
    File::open(
        executable_context.virtual_file_system,
        task,
        Path::REAL_TIME_CLOCK,
        access.into(),
    )
    .await
    .map_err(Error::FailedToOpenFile)
}

/// Reads the clock through `GET_TIME`.
async fn get_time<C: CommandContext>(context: &C) -> Result<(File<'static>, RealTimeClockTimeAbi)> {
    let mut file = open_device(context, AccessFlags::Read).await?;
    let request = RealTimeClockTimeAbi {
        version: ABI_VERSION,
        valid: VALID,
        reserved: 0,
        nanoseconds: 0,
        seconds: 0,
    };
    let answer = file
        .control(GET_TIME, &request)
        .await
        .map_err(Error::FailedToControlDevice)?;
    Ok((file, answer))
}

fn format_time(seconds: i64, nanoseconds: u32) -> alloc::string::String {
    civil_time::format_civil(civil_time::civil_from_unix(seconds, nanoseconds))
}

async fn show<C: CommandContext>(context: &mut C) -> Result<()> {
    let (_file, answer) = get_time(context).await?;
    if answer.valid == NOT_VALID {
        context
            .write_out_line(b"The real-time clock holds no valid time")
            .await?;
    } else {
        context
            .write_out_fmt(format_args!(
                "{} UTC\n",
                format_time(answer.seconds, answer.nanoseconds)
            ))
            .await?;
    }
    Ok(())
}

/// Sets the clock from the system UTC time through `SET_TIME`. The read-write
/// open already restricts this to root.
async fn system_to_clock<C: CommandContext>(context: &mut C) -> Result<()> {
    let manager = context.executable_context().time_manager;
    let timestamp = manager.realtime_now().map_err(Error::Time)?;
    let nanos = timestamp.as_nanos();
    let seconds = i64::try_from(nanos.div_euclid(civil_time::NANOS_PER_SECOND))
        .map_err(|_| Error::InvalidDateTime)?;
    let nanoseconds = nanos.rem_euclid(civil_time::NANOS_PER_SECOND) as u32;

    let mut file = open_device(context, AccessFlags::READ_WRITE).await?;
    let request = RealTimeClockTimeAbi {
        version: ABI_VERSION,
        valid: VALID,
        reserved: 0,
        nanoseconds,
        seconds,
    };
    let answer = file
        .control(SET_TIME, &request)
        .await
        .map_err(Error::FailedToControlDevice)?;
    if answer.valid == NOT_VALID {
        return Err(Error::RealTimeClockNotValid);
    }
    context
        .write_out_fmt(format_args!(
            "Real-time clock set to {} UTC\n",
            format_time(answer.seconds, answer.nanoseconds)
        ))
        .await?;
    Ok(())
}

/// Sets the system UTC time from the clock, as root only: the sample is
/// recorded with [`ClockSource::Rtc`], so the write-back task never stores it
/// back into the same clock.
async fn clock_to_system<C: CommandContext>(context: &mut C) -> Result<()> {
    let (_file, answer) = get_time(context).await?;
    if answer.valid != VALID {
        return Err(Error::RealTimeClockNotValid);
    }
    let executable_context = context.executable_context();
    let user = executable_context
        .task_manager
        .get_user(context.task_id())
        .await
        .map_err(Error::FailedToGetTaskUser)?;
    if !xila::users::Manager::is_root(user) {
        return Err(Error::NotRoot);
    }
    let manager: &Manager<'_> = executable_context.time_manager;
    manager
        .set_realtime(RealtimeSample {
            timestamp: Timestamp::from_nanos(
                i128::from(answer.seconds) * civil_time::NANOS_PER_SECOND
                    + i128::from(answer.nanoseconds),
            ),
            source: ClockSource::Rtc,
            // The device contract carries no resolution field; one second is a
            // safe quantization for any RTC chip.
            resolution: Duration::from_secs(1),
            accuracy: None,
        })
        .map_err(Error::Time)?;
    context
        .write_out_fmt(format_args!(
            "UTC set from the real-time clock to {} UTC\n",
            format_time(answer.seconds, answer.nanoseconds)
        ))
        .await?;
    Ok(())
}
