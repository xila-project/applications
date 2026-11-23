use core::{
    fmt::Display,
    num::{NonZeroU8, NonZeroUsize},
};

pub type Result<T> = core::result::Result<T, Error>;

use xila::{
    authentication, executable, graphics, internationalization::t, task, virtual_file_system,
};

/// Why the desk could not read its shortcut directory: the error of the
/// creation attempt, when it failed, together with the error of the opening
/// attempt that made the directory unreadable.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ShortcutDirectoryFailure {
    pub creation: Option<virtual_file_system::IoErrorKind>,
    pub opening: virtual_file_system::IoErrorKind,
}

// Written by hand so that the logged error names the directory.
impl core::fmt::Debug for ShortcutDirectoryFailure {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ShortcutDirectoryFailure")
            .field("path", &crate::shortcut::SHORTCUT_PATH.as_str())
            .field("creation", &self.creation)
            .field("opening", &self.opening)
            .finish()
    }
}

#[derive(Debug, Clone)]
#[repr(u8)]
pub enum Error {
    Graphics(graphics::Error) = 1,
    FailedToCreateObject,
    FailedToGetChild,
    FailedToSetEnvironmentVariable(task::Error),
    InvalidUtf8(core::str::Utf8Error),
    AuthenticationFailed(authentication::Error),
    FailedToSetTaskUser(task::Error),
    FailedToDeserializeShortcut(miniserde::Error),
    FailedToGetCurrentTaskIdentifier(task::Error),
    FailedToReadShortcutDirectory(ShortcutDirectoryFailure),
    FailedToGetShortcutFilePath,
    FailedToReadShortcutFile(virtual_file_system::IoErrorKind),
    FailedToOpenStandardFile(executable::Error),
    FailedToExecuteShortcut(executable::Error),
    NullCharacterInString(alloc::ffi::NulError),
    MissingArguments,
    InvalidOption,
    InvalidNumberOfArguments,
    FailedToAddShortcut(virtual_file_system::IoErrorKind),
    FailedToOpenDirectory(virtual_file_system::IoErrorKind),
}

impl Error {
    pub fn get_discriminant(&self) -> NonZeroU8 {
        unsafe { *(self as *const Self as *const NonZeroU8) }
    }
}

impl From<Error> for NonZeroUsize {
    fn from(error: Error) -> Self {
        error.get_discriminant().into()
    }
}

impl From<graphics::Error> for Error {
    fn from(error: graphics::Error) -> Self {
        Error::Graphics(error)
    }
}

impl Display for Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            Self::Graphics(error) => {
                write!(formatter, "{}", t!("errors.graphics_error", arg0 = error))
            }
            Self::FailedToCreateObject => {
                write!(formatter, "{}", t!("errors.failed_to_create_object"))
            }
            Self::FailedToGetChild => {
                write!(formatter, "{}", t!("errors.failed_to_get_child"))
            }
            Self::FailedToSetEnvironmentVariable(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_set_environment_variable", arg0 = error)
                )
            }
            Self::InvalidUtf8(error) => {
                write!(formatter, "{}", t!("errors.invalid_utf_8", arg0 = error))
            }
            Self::AuthenticationFailed(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.authentication_failed", arg0 = error)
                )
            }
            Self::FailedToSetTaskUser(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_set_task_user", arg0 = error)
                )
            }
            Self::FailedToDeserializeShortcut(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_deserialize_shortcut", arg0 = error)
                )
            }
            Self::FailedToGetCurrentTaskIdentifier(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_get_current_task_identifier", arg0 = error)
                )
            }
            Self::FailedToReadShortcutDirectory(failure) => {
                write!(
                    formatter,
                    "{}",
                    t!(
                        "errors.failed_to_read_shortcut_directory",
                        arg0 = failure.opening
                    )
                )
            }
            Self::FailedToGetShortcutFilePath => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_get_shortcut_file_path")
                )
            }
            Self::FailedToReadShortcutFile(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_read_shortcut_file", arg0 = error)
                )
            }
            Self::FailedToOpenStandardFile(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_open_standard_file", arg0 = error)
                )
            }
            Self::FailedToExecuteShortcut(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_execute_shortcut", arg0 = error)
                )
            }
            Self::NullCharacterInString(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.null_character_in_string", arg0 = error)
                )
            }
            Self::MissingArguments => {
                write!(formatter, "{}", t!("errors.missing_arguments"))
            }
            Self::InvalidOption => {
                write!(formatter, "Invalid option")
            }
            Self::InvalidNumberOfArguments => {
                write!(formatter, "Invalid number of arguments")
            }
            Self::FailedToAddShortcut(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_add_shortcut", arg0 = error)
                )
            }
            Self::FailedToOpenDirectory(error) => {
                write!(
                    formatter,
                    "{}",
                    t!("errors.failed_to_open_directory", arg0 = error)
                )
            }
        }
    }
}
