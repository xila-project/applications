use core::fmt::Display;
use core::num::{NonZeroU16, NonZeroUsize};

use alloc::fmt;
#[cfg(feature = "network")]
use xila::network;
use xila::virtual_file_system;
use xila::{authentication, internationalization::t, lifecycle, task};

pub type Result<T, E = Error<'static>> = core::result::Result<T, E>;

#[derive(Debug, Clone)]
#[repr(u16)]
pub enum Error<'a> {
    AuthenticationFailed(authentication::Error) = 1,
    FailedToSetTaskUser(task::Error),
    FailedToSetEnvironmentVariable(task::Error),
    FailedToSetCurrentDirectory(task::Error),
    FailedToRemoveEnvironmentVariable(task::Error),
    FailedToReadEnvironmentVariable(task::Error),
    FailedToTokenizeCommandLine,
    MissingFileNameAfterRedirectOut,
    MissingFileNameAfterRedirectIn,
    MissingCommand,
    CommandNotFound,
    FailedToGetTaskIdentifier,
    InvalidPath,
    FailedToGetPath,
    FailedToExecuteCommand,
    FailedToJoinTask,
    InvalidNumberOfArguments,
    FailedToJoinPath,
    FailedToCreateDirectory(virtual_file_system::IoErrorKind),
    FailedToRemoveDirectory(virtual_file_system::IoErrorKind),
    FailedToOpenDirectory(virtual_file_system::IoErrorKind),
    FailedToOpenFile(virtual_file_system::IoErrorKind),
    FailedToReadFile(virtual_file_system::IoErrorKind),
    RequiresValue,
    DoesNotRequireValue,
    ArgumentParsing(getargs_derive::Error<'a>),
    InvalidArgument,
    MissingPositionalArgument(&'static str),
    InvalidOption,
    FailedToGetMetadata(virtual_file_system::IoErrorKind),
    FailedToReadDirectoryEntry(virtual_file_system::IoErrorKind),
    #[cfg(feature = "network")]
    FailedToResolve(network::ResolveError),
    #[cfg(feature = "network")]
    FailedToCreateSocket(network::IoErrorKind),
    Format,
    InvalidSleepDuration,
    /// The value is neither a valid POSIX TZ string nor a zone name of the build.
    InvalidTimeZoneSetting,
    FailedToSetTimeZone(virtual_file_system::IoErrorKind),
    FailedToSetPermissions(virtual_file_system::IoErrorKind),
    FailedToGetTaskUser(task::Error),
    ExitStatus(NonZeroUsize),
    Syntax(crate::tokenizer::SyntaxError),
    FailedToWriteFile(virtual_file_system::IoErrorKind),
    LineTooLong,
    FailedToRename(virtual_file_system::IoErrorKind),
    FailedToCreateSymbolicLink(virtual_file_system::IoErrorKind),
    FailedToReadSymbolicLink(virtual_file_system::IoErrorKind),
    Lifecycle(lifecycle::Error),
    /// Standard input supports terminal attributes but echo could not be
    /// turned off, so the password is not read and the login session ends.
    FailedToHidePassword(virtual_file_system::IoErrorKind),
    /// Echo stays off after the password was read.
    FailedToRestoreTerminalEcho(virtual_file_system::IoErrorKind),
    #[cfg(feature = "network")]
    NetworkUnavailable,
    /// `sudo` was given no command, an unknown option or `-u` without a user.
    SudoUsage,
    /// The caller is not a member of the administrator group.
    SudoNotAllowed,
    SudoUnknownUser(alloc::string::String),
    /// `sudo` runs external commands only.
    SudoBuiltIn(alloc::string::String),
    /// Too many failed attempts: the caller must wait this many seconds.
    SudoLockedOut(u64),
    /// Standard input ended or the task was asked to stop during the prompt.
    SudoAborted,
    SudoFailedToExecute(xila::executable::Error),
    /// The command is neither a name nor an absolute path of an entry of the
    /// secure path.
    SudoCommandNotAllowed(alloc::string::String),
    /// Another `sudo` attempt of the same user is in progress.
    SudoBusy,
    /// Standard input cannot turn echo off, or echo stayed on, so the
    /// password is not prompted for.
    SudoEchoNotSuppressed(virtual_file_system::IoErrorKind),
    /// The password line is longer than the maximum; it was read to its end.
    SudoPasswordTooLong,
    /// The argument is not an ISO 8601 UTC date and time.
    #[cfg(feature = "time-commands")]
    InvalidDateTime,
    /// The given time is earlier than the build date of this firmware.
    #[cfg(feature = "time-commands")]
    BeforeBuildDate,
    /// The command is reserved to the root user.
    #[cfg(feature = "time-commands")]
    NotRoot,
    /// `TZ` is set but is not a valid POSIX time zone rule string.
    #[cfg(feature = "time-commands")]
    InvalidTimeZone(xila::time_zone::ParseError),
    /// The real-time clock holds no valid time.
    #[cfg(feature = "time-commands")]
    RealTimeClockNotValid,
    /// Unknown subcommand of a command that takes one.
    #[cfg(feature = "time-commands")]
    InvalidSubcommand(&'static str),
    /// A control command on a device node failed.
    #[cfg(feature = "time-commands")]
    FailedToControlDevice(virtual_file_system::IoErrorKind),
    /// The time manager could not serve the request.
    #[cfg(feature = "time-commands")]
    Time(xila::time::Error),
    /// The privileged manual UTC set failed.
    #[cfg(feature = "time-commands")]
    ManualTime(xila::time::manual::ManualError),
    #[cfg(feature = "crash-command")]
    CrashReports(virtual_file_system::IoErrorKind),
}

impl<A: getargs::Argument> From<getargs::Error<A>> for Error<'_> {
    fn from(value: getargs::Error<A>) -> Self {
        match value {
            getargs::Error::RequiresValue(_) => Error::RequiresValue,
            getargs::Error::DoesNotRequireValue(_) => Error::DoesNotRequireValue,
            _ => Error::InvalidOption,
        }
    }
}

impl<'a> From<getargs_derive::Error<'a>> for Error<'a> {
    fn from(value: getargs_derive::Error<'a>) -> Self {
        Error::ArgumentParsing(value)
    }
}

impl Error<'_> {
    pub fn get_discriminant(&self) -> NonZeroU16 {
        unsafe { *<*const _>::from(self).cast::<NonZeroU16>() }
    }
}

impl From<fmt::Error> for Error<'_> {
    fn from(_: fmt::Error) -> Self {
        Error::Format
    }
}

impl Display for Error<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Error::ExitStatus(status) => write!(formatter, "{status}"),
            Error::Syntax(error) => write!(
                formatter,
                "{}",
                t!(
                    "errors.syntax_error_at_byte",
                    arg0 = error.position,
                    arg1 = error.message
                )
            ),
            Error::FailedToWriteFile(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_write_file", arg0 = error)
                )
            }
            Error::LineTooLong => write!(formatter, "{}", t!("errors.line_too_long")),
            Error::FailedToRename(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_rename_path", arg0 = error)
                )
            }
            Error::FailedToCreateSymbolicLink(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_create_symbolic_link", arg0 = error)
                )
            }
            Error::FailedToReadSymbolicLink(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_read_symbolic_link", arg0 = error)
                )
            }
            Error::Lifecycle(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.lifecycle_request_failed", arg0 = error)
                )
            }
            Error::FailedToHidePassword(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_hide_password", arg0 = error)
                )
            }
            Error::FailedToRestoreTerminalEcho(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_restore_terminal_echo", arg0 = error)
                )
            }
            Error::AuthenticationFailed(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.authentication_failed", arg0 = error)
                )
            }
            Error::FailedToSetTaskUser(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_set_task_user", arg0 = error)
                )
            }
            Error::FailedToSetEnvironmentVariable(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_set_environment_variable", arg0 = error)
                )
            }
            Error::FailedToTokenizeCommandLine => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_tokenize_command_line")
                )
            }
            Error::MissingFileNameAfterRedirectOut => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.missing_file_name_after_redirect_out")
                )
            }
            Error::MissingFileNameAfterRedirectIn => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.missing_file_name_after_redirect_in")
                )
            }
            Error::MissingCommand => {
                write!(formatter, "{}", t!("execution.errors.missing_command"))
            }
            Error::CommandNotFound => {
                write!(formatter, "{}", t!("execution.errors.command_not_found"))
            }
            Error::FailedToGetTaskUser(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_get_task_user", arg0 = error)
                )
            }
            Error::FailedToGetTaskIdentifier => {
                write!(formatter, "{}", t!("errors.failed_to_get_task_identifier"))
            }
            Error::InvalidPath => write!(formatter, "{}", t!("errors.invalid_path")),
            Error::FailedToGetPath => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_get_environment_variable")
                )
            }
            Error::FailedToExecuteCommand => {
                write!(formatter, "{}", t!("errors.failed_to_execute_command"))
            }
            Error::FailedToJoinTask => {
                write!(formatter, "{}", t!("errors.failed_to_join_task"))
            }
            Error::InvalidNumberOfArguments => {
                write!(formatter, "{}", t!("errors.invalid_number_of_arguments"))
            }
            Error::FailedToRemoveEnvironmentVariable(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_remove_environment_variable", arg0 = error)
                )
            }
            Error::FailedToJoinPath => {
                write!(formatter, "{}", t!("errors.failed_to_join_path"))
            }
            Error::FailedToCreateDirectory(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_create_directory", arg0 = error)
                )
            }
            Error::FailedToRemoveDirectory(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_remove_directory", arg0 = error)
                )
            }
            Error::FailedToOpenDirectory(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_open_directory", arg0 = error)
                )
            }
            Error::FailedToOpenFile(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_open_file", arg0 = error)
                )
            }
            Error::FailedToReadFile(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_read_file", arg0 = error)
                )
            }
            Error::RequiresValue => {
                write!(formatter, "{}", t!("errors.option_requires_a_value"))
            }
            Error::DoesNotRequireValue => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.option_does_not_require_a_value")
                )
            }
            #[cfg(feature = "crash-command")]
            Error::CrashReports(error) => {
                write!(formatter, "{}", t!("errors.crash_reports", arg0 = error))
            }
            Error::InvalidArgument => {
                write!(formatter, "{}", t!("errors.invalid_argument"))
            }
            Error::MissingPositionalArgument(name) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.missing_positional_argument", arg0 = name)
                )
            }
            Error::InvalidOption => {
                write!(formatter, "{}", t!("errors.invalid_option"))
            }
            Error::FailedToGetMetadata(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_get_metadata", arg0 = error)
                )
            }
            Error::FailedToSetCurrentDirectory(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_set_current_directory", arg0 = error)
                )
            }
            Error::FailedToReadDirectoryEntry(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_read_directory_entry", arg0 = error)
                )
            }
            #[cfg(feature = "network")]
            Error::NetworkUnavailable => {
                write!(formatter, "{}", t!("errors.network_unavailable"))
            }
            #[cfg(feature = "network")]
            Error::FailedToResolve(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_resolve_domain", arg0 = error)
                )
            }
            #[cfg(feature = "network")]
            Error::FailedToCreateSocket(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_create_socket", arg0 = error)
                )
            }
            Error::ArgumentParsing(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.argument_parsing_error", arg0 = error)
                )
            }
            Error::InvalidSleepDuration => {
                write!(formatter, "{}", t!("sleep.errors.invalid_duration"))
            }
            Error::InvalidTimeZoneSetting => {
                write!(formatter, "{}", t!("time_zone.errors.invalid"))
            }
            Error::FailedToSetTimeZone(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("time_zone.errors.failed_to_set", arg0 = error)
                )
            }
            Error::FailedToSetPermissions(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_set_permissions", arg0 = error)
                )
            }
            Error::Format => {
                write!(formatter, "{}", t!("errors.format_failed"))
            }
            Error::SudoUsage => write!(formatter, "{}", t!("sudo.errors.usage")),
            Error::SudoNotAllowed => write!(formatter, "{}", t!("sudo.errors.not_allowed")),
            Error::SudoUnknownUser(user) => write!(
                formatter,
                "{}",
                t!("sudo.errors.unknown_user", arg0 = user.as_str())
            ),
            Error::SudoBuiltIn(command) => write!(
                formatter,
                "{}",
                t!("sudo.errors.built_in", arg0 = command.as_str())
            ),
            Error::SudoLockedOut(seconds) => write!(
                formatter,
                "{}",
                t!("sudo.errors.locked_out", arg0 = seconds)
            ),
            Error::SudoAborted => write!(formatter, "{}", t!("sudo.errors.aborted")),
            Error::SudoFailedToExecute(error) => write!(
                formatter,
                "{}",
                t!("sudo.errors.failed_to_execute", arg0 = error)
            ),
            Error::SudoCommandNotAllowed(command) => write!(
                formatter,
                "{}",
                t!("sudo.errors.command_not_allowed", arg0 = command.as_str())
            ),
            Error::SudoBusy => write!(formatter, "{}", t!("sudo.errors.busy")),
            Error::SudoEchoNotSuppressed(error) => write!(
                formatter,
                "{}",
                t!("sudo.errors.echo_not_suppressed", arg0 = error)
            ),
            Error::SudoPasswordTooLong => {
                write!(formatter, "{}", t!("sudo.errors.password_too_long"))
            }
            #[cfg(feature = "time-commands")]
            Error::InvalidDateTime => write!(formatter, "{}", t!("errors.invalid_date_time")),
            #[cfg(feature = "time-commands")]
            Error::BeforeBuildDate => write!(formatter, "{}", t!("errors.before_build_date")),
            #[cfg(feature = "time-commands")]
            Error::NotRoot => write!(formatter, "{}", t!("errors.not_root")),
            #[cfg(feature = "time-commands")]
            Error::InvalidTimeZone(error) => write!(formatter, "{error}"),
            #[cfg(feature = "time-commands")]
            Error::RealTimeClockNotValid => {
                write!(formatter, "{}", t!("errors.real_time_clock_not_valid"))
            }
            #[cfg(feature = "time-commands")]
            Error::InvalidSubcommand(command) => write!(
                formatter,
                "{}",
                t!("errors.invalid_subcommand", arg0 = command)
            ),
            #[cfg(feature = "time-commands")]
            Error::FailedToControlDevice(error) => write!(
                formatter,
                "{}",
                t!("errors.failed_to_control_device", arg0 = error)
            ),
            #[cfg(feature = "time-commands")]
            Error::Time(error) => write!(formatter, "{error}"),
            #[cfg(feature = "time-commands")]
            Error::ManualTime(error) => write!(formatter, "{error}"),
            Error::FailedToReadEnvironmentVariable(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_read_environment_variable", arg0 = error)
                )
            }
        }
    }
}

impl From<Error<'_>> for NonZeroUsize {
    fn from(error: Error<'_>) -> Self {
        match error {
            Error::ExitStatus(status) => status,
            error => error.get_discriminant().into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;
    use alloc::string::String;

    #[test]
    fn parser_error_keeps_the_borrowed_argument() {
        let input = String::from("not-a-number");
        let error: Error<'_> = getargs_derive::Error::InvalidValue {
            name: "--lines <LINES>",
            value: input.as_str(),
        }
        .into();
        let Error::ArgumentParsing(getargs_derive::Error::InvalidValue { value, .. }) = error
        else {
            panic!("parser error was not preserved");
        };
        assert_eq!(value.as_ptr(), input.as_ptr());
    }
}
