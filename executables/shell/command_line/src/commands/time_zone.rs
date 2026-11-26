//! `time_zone [-l] [VALUE]`: show the time zone of the Shell, list the zone
//! names of the build, or set the system time zone. Built with the
//! `time-zone-command` feature.
//!
//! VALUE is a POSIX TZ string (`CET-1CEST,M3.5.0,M10.5.0/3`), or an IANA zone
//! name (`Europe/Paris`) when the build links the table of its region; a name
//! is stored with its TZ string. Setting needs root or the administrator group
//! and goes through `xila::local_time::set`: programs started afterwards get
//! the new `TZ`, and this Shell too when it still follows the system value.
//!
//! The display reads the `TZ` variable of the Shell, not the configuration
//! file: it shows the time zone that the Shell and the programs it starts use.

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
        let mut output = alloc::string::String::new();

        if arguments.list {
            let count = iana::zone_count();
            if count == 0 {
                writeln!(output, "{}", t!("time_zone.no_names"))?;
            }
            for index in 0..count {
                if let Some(name) = iana::zone_name(index) {
                    output.push_str(name);
                    output.push('\n');
                }
            }
            context.write_out(output.as_bytes()).await?;
            return Ok(());
        }

        if !arguments.value.is_empty() {
            let setting =
                setting_for(arguments.value).map_err(|_| Error::InvalidTimeZoneSetting)?;
            local_time::set(
                executable.task_manager,
                executable.users_manager,
                executable.virtual_file_system,
                task,
                &setting,
            )
            .await
            .map_err(Error::FailedToSetTimeZone)?;
            writeln!(output, "{}", t!("time_zone.saved"))?;
        }

        let time_zone = executable
            .task_manager
            .get_environment_variable(task, local_time::ENVIRONMENT_VARIABLE)
            .await
            .ok();
        let time_zone = time_zone
            .as_ref()
            .map_or(local_time::DEFAULT_TIME_ZONE, |variable| {
                variable.get_value()
            });
        let zone =
            local_time::time_zone::TimeZone::parse(time_zone).unwrap_or_else(|_| local_time::utc());
        write!(
            output,
            "{}: {time_zone}\n{}: ",
            t!("time_zone.labels.string"),
            t!("time_zone.labels.local"),
        )?;
        match executable
            .time_manager
            .realtime_now()
            .and_then(|utc| utc.try_as_seconds())
        {
            Ok(utc) => local_time::write_local_time(&mut output, &zone, utc)?,
            Err(_) => output.push_str(t!("time_zone.unsynchronized")),
        }
        output.push('\n');
        context.write_out(output.as_bytes()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        let custom = setting_for("<+0545>-5:45").unwrap();
        assert_eq!(custom.name(), None);
        assert!(setting_for("UTC").is_err());
        // A name resolves only when the build links its region.
        assert_eq!(
            setting_for("Europe/Paris").is_ok(),
            iana::contains("Europe/Paris")
        );
    }
}
