//! `time_zone [-l] [VALUE]`: show the system time zone, list the zone names of
//! the build, or set the time zone.
//!
//! VALUE is a POSIX TZ string (`CET-1CEST,M3.5.0,M10.5.0/3`), or an IANA zone
//! name (`Europe/Paris`) when the build links the table of its region; a name
//! is stored with its TZ string. Setting needs root or the administrator group
//! and goes through `xila::local_time::set`: programs started afterwards get
//! the new `TZ`, and this Shell too when it still follows the system value.

use core::fmt::Write;
use getargs_derive::GetArgs;
use xila::file_system::Path;
use xila::internationalization::t;
use xila::local_time::{self, Setting};
use xila::time_zone::iana;

use super::{CommandContext, UserCommand};
use crate::{Error, Result};

pub struct TimeZoneCommand;

#[derive(GetArgs)]
#[command(
    name = "time_zone",
    about = "Show the time zone, or set it to a POSIX TZ string (for example CET-1CEST,M3.5.0,M10.5.0/3) or to a zone name that this build has (see -l). Setting needs root or the administrator group."
)]
pub(super) struct TimeZoneArguments<'a> {
    #[arg(default_value = "")]
    value: &'a str,
    /// List the zone names of this build.
    #[arg(short = 'l', long = "list")]
    list: bool,
}

/// The setting for a value: an IANA name of the linked tables with its TZ
/// string, or a TZ string without a name.
pub(crate) fn setting_for(value: &str) -> core::result::Result<Setting, local_time::SettingError> {
    match iana::time_zone_string(value) {
        Some(time_zone) => Setting::new(Some(value), time_zone),
        None => Setting::new(None, value),
    }
}

/// Write the local time of `utc` in `setting`, for example
/// `2026-03-29 03:00:00 CEST (UTC+02:00)`.
pub(crate) fn write_local_time(
    output: &mut impl Write,
    setting: &Setting,
    utc: Option<i64>,
) -> core::fmt::Result {
    let Some(utc) = utc else {
        return write!(output, "{}", t!("time_zone.unsynchronized"));
    };
    let zone = setting.time_zone();
    let local = zone.from_utc(utc);
    let date_time = local.date_time;
    write!(
        output,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {} (UTC",
        date_time.year,
        date_time.month,
        date_time.day,
        date_time.hour,
        date_time.minute,
        date_time.second,
        local.abbreviation,
    )?;
    local_time::write_offset(output, local.offset_seconds)?;
    output.write_char(')')
}

impl UserCommand for TimeZoneCommand {
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
        let arguments = TimeZoneArguments::parse(options)?;
        let executable = context.executable_context();
        let task = context.task_id();

        if arguments.list {
            if iana::zone_count() == 0 {
                context
                    .write_out_fmt(format_args!("{}\n", t!("time_zone.no_names")))
                    .await?;
            }
            for index in 0..iana::zone_count() {
                if let Some(name) = iana::zone_name(index) {
                    context.write_out_fmt(format_args!("{name}\n")).await?;
                }
            }
            return Ok(());
        }

        if !arguments.value.is_empty() {
            let setting = setting_for(arguments.value).map_err(|_| Error::InvalidTimeZone)?;
            local_time::set(
                executable.task_manager,
                executable.users_manager,
                executable.virtual_file_system,
                task,
                &setting,
            )
            .await
            .map_err(Error::FailedToSetTimeZone)?;
            context
                .write_out_fmt(format_args!("{}\n", t!("time_zone.saved")))
                .await?;
        }

        let setting = local_time::load(executable.virtual_file_system, task).await;
        let utc = executable
            .time_manager
            .realtime_now()
            .and_then(|utc| utc.try_as_seconds())
            .ok();
        let mut local = alloc::string::String::new();
        write_local_time(&mut local, &setting, utc)?;
        let name = setting.name().unwrap_or(t!("time_zone.custom"));
        context
            .write_out_fmt(format_args!(
                "{}: {name}\n{}: {}\n{}: {local}\n",
                t!("time_zone.labels.name"),
                t!("time_zone.labels.string"),
                setting.time_zone_string(),
                t!("time_zone.labels.local"),
            ))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn values_and_local_time() {
        let custom = setting_for("<+0545>-5:45").unwrap();
        assert_eq!(custom.name(), None);
        assert!(setting_for("UTC").is_err());
        // A name resolves only when the build links its region.
        assert_eq!(
            setting_for("Europe/Paris").is_ok(),
            iana::contains("Europe/Paris")
        );

        let paris = Setting::new(None, "CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let mut output = String::new();
        write_local_time(&mut output, &paris, Some(1_774_746_000)).unwrap();
        assert_eq!(output, "2026-03-29 03:00:00 CEST (UTC+02:00)");
        output.clear();
        write_local_time(&mut output, &paris, Some(1_792_890_000)).unwrap();
        assert_eq!(output, "2026-10-25 02:00:00 CET (UTC+01:00)");
    }
}
