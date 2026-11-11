use core::fmt::Display;
use core::num::{NonZeroU16, NonZeroUsize};

use alloc::fmt;
#[cfg(feature = "network")]
use xila::network;
use xila::virtual_file_system;
use xila::{authentication, internationalization::t, task};

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
    FailedToCreateDirectory(virtual_file_system::Error),
    FailedToRemoveDirectory(virtual_file_system::Error),
    FailedToOpenDirectory(virtual_file_system::Error),
    FailedToOpenFile(virtual_file_system::Error),
    FailedToReadFile(virtual_file_system::Error),
    RequiresValue,
    DoesNotRequireValue,
    ArgumentParsing(getargs_derive::Error<'a>),
    InvalidArgument,
    MissingPositionalArgument(&'static str),
    InvalidOption,
    FailedToGetMetadata(virtual_file_system::Error),
    FailedToReadDirectoryEntry(virtual_file_system::Error),
    #[cfg(feature = "network")]
    FailedToResolve(network::Error),
    #[cfg(feature = "network")]
    FailedToCreateSocket(network::Error),
    Format,
    InvalidSleepDuration,
    FailedToSetPermissions(virtual_file_system::Error),
    FailedToGetTaskUser(task::Error),
    ExitStatus(NonZeroUsize),
    Syntax(crate::tokenizer::SyntaxError),
    FailedToWriteFile(virtual_file_system::Error),
    LineTooLong,
    FailedToRename(virtual_file_system::Error),
    FailedToCreateSymbolicLink(virtual_file_system::Error),
    FailedToReadSymbolicLink(virtual_file_system::Error),
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
