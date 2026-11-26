//! `date`: show the current UTC time (and the local time when the task's `TZ`
//! variable is set), plus the UTC synchronization status and source; or, as
//! root, set UTC from an ISO 8601 UTC time with `--set`.

use alloc::format;
use getargs_derive::GetArgs;
use xila::file_system::Path;
use xila::task::TaskIdentifier;
use xila::time::{self, ClockSource, Manager, Timestamp};

use super::{CommandContext, UserCommand, civil_time};
use crate::{Error, Result};

pub struct DateCommand;

#[derive(GetArgs)]
#[command(
    name = "date",
    about = "Show the current UTC time, the local time when TZ is set, and the UTC synchronization status; or set UTC as root with --set TIME (ISO 8601 UTC, written back to the real-time clock). Setting the time needs a root login: sudo runs external commands only."
)]
pub(super) struct DateArguments<'a> {
    /// Set UTC from an ISO 8601 UTC time, for example 2026-11-26T12:34:56Z (root only, not earlier than the build date).
    #[arg(long = "set")]
    set: Option<&'a str>,
}

const fn parse_decimal(text: &str) -> u64 {
    let digits = text.as_bytes();
    assert!(!digits.is_empty(), "expected a decimal number");
    let mut value: u64 = 0;
    let mut index = 0;
    while index < digits.len() {
        let digit = digits[index];
        assert!(digit.is_ascii_digit(), "expected a decimal number");
        value = value * 10 + (digit - b'0') as u64;
        index += 1;
    }
    value
}

/// The build date of this firmware image in Unix seconds: the floor for a
/// manual UTC set, matching the floor the firmware's real-time clock uses. A
/// time earlier than it would seed UTC but never reach the clock.
const BUILD_DATE_FLOOR: i64 = parse_decimal(env!("XILA_BUILD_EPOCH")) as i64;

impl UserCommand for DateCommand {
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
        let arguments = DateArguments::parse(options)?;
        let executable_context = context.executable_context();
        let manager = executable_context.time_manager;
        let task = context.task_id();
        match arguments.set {
            Some(value) => {
                set_utc(
                    context,
                    executable_context.task_manager,
                    task,
                    manager,
                    value,
                )
                .await
            }
            None => show(context, manager, task).await,
        }
    }
}

/// Splits a timestamp into signed Unix seconds and nanoseconds within the
/// second.
fn split_timestamp(timestamp: Timestamp) -> Result<(i64, u32)> {
    let nanos = timestamp.as_nanos();
    let seconds = nanos.div_euclid(civil_time::NANOS_PER_SECOND);
    let nanoseconds = nanos.rem_euclid(civil_time::NANOS_PER_SECOND) as u32;
    i64::try_from(seconds)
        .map(|seconds| (seconds, nanoseconds))
        .map_err(|_| Error::Time(time::Error::Overflow))
}

async fn set_utc<C: CommandContext>(
    context: &mut C,
    task_manager: &xila::task::Manager,
    task: TaskIdentifier,
    manager: &Manager<'_>,
    value: &str,
) -> Result<()> {
    let Some((seconds, nanoseconds)) = civil_time::parse_iso8601_utc(value) else {
        return Err(Error::InvalidDateTime);
    };
    if seconds < BUILD_DATE_FLOOR {
        return Err(Error::BeforeBuildDate);
    }
    let timestamp = Timestamp::from_nanos(
        i128::from(seconds) * civil_time::NANOS_PER_SECOND + i128::from(nanoseconds),
    );
    time::manual::set_realtime_manual(task_manager, task, manager, timestamp)
        .await
        .map_err(Error::ManualTime)?;
    context
        .write_out_fmt(format_args!(
            "UTC set to {} UTC\n",
            civil_time::format_civil(civil_time::civil_from_unix(seconds, nanoseconds))
        ))
        .await?;
    Ok(())
}

async fn show<C: CommandContext>(
    context: &mut C,
    manager: &Manager<'_>,
    task: TaskIdentifier,
) -> Result<()> {
    if let Ok(timestamp) = manager.realtime_now() {
        let (seconds, nanoseconds) = split_timestamp(timestamp)?;
        context
            .write_out_fmt(format_args!(
                "{} UTC\n",
                civil_time::format_civil(civil_time::civil_from_unix(seconds, nanoseconds))
            ))
            .await?;
        show_local_time(context, task, seconds, nanoseconds).await?;
    }
    match manager.synchronization() {
        None => context.write_out_line(b"UTC is unsynchronized").await?,
        Some(synchronization) => {
            let source = match synchronization.source {
                ClockSource::Host => "host",
                ClockSource::Rtc => "real-time clock",
                ClockSource::BuildDate => "build date (fallback)",
                ClockSource::Manual => "manual",
                ClockSource::Network => "network",
            };
            context
                .write_out_fmt(format_args!("UTC source: {source}\n"))
                .await?;
        }
    }
    Ok(())
}

/// Prints the local time when the task's `TZ` variable is set. An unparseable
/// `TZ` only downgrades the display to UTC: a warning goes to standard error
/// and the command continues.
async fn show_local_time<C: CommandContext>(
    context: &mut C,
    task: TaskIdentifier,
    seconds: i64,
    nanoseconds: u32,
) -> Result<()> {
    let task_manager = context.executable_context().task_manager;
    let Ok(variable) = task_manager.get_environment_variable(task, "TZ").await else {
        return Ok(());
    };
    let zone = match xila::time_zone::TimeZone::parse(variable.get_value()) {
        Ok(zone) => zone,
        Err(error) => {
            let _ = crate::execution::write_all(
                context.standard().error(),
                format!(
                    "date: warning: TZ is not a valid POSIX time zone rule string ({error}): {}\n",
                    variable.get_value()
                )
                .as_bytes(),
            )
            .await;
            return Ok(());
        }
    };
    let local = zone.from_utc(seconds);
    let offset = local.offset_seconds;
    let sign = if offset < 0 { '-' } else { '+' };
    let magnitude = offset.abs();
    let date_time = local.date_time;
    context
        .write_out_fmt(format_args!(
            "{} {sign}{:02}:{:02} {} (local time, TZ={})\n",
            civil_time::format_civil(civil_time::CivilTime {
                year: date_time.year,
                month: date_time.month,
                day: date_time.day,
                hour: date_time.hour,
                minute: date_time.minute,
                second: date_time.second,
                nanoseconds,
            }),
            magnitude / 3_600,
            (magnitude % 3_600) / 60,
            local.abbreviation,
            variable.get_value()
        ))
        .await?;
    Ok(())
}
