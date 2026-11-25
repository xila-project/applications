//! Errors of the Shortcuts application.

use core::fmt;

/// What can go wrong while reading the files or talking to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The configuration or profiles file could not be read.
    Storage,
    /// A file is larger than the largest size the application reads.
    TooLarge,
    /// The settings file holds an unknown or malformed value.
    Configuration,
    /// The profiles file holds an unknown or malformed value.
    Profiles,
    /// `/devices/human_interface/emulation` does not exist on this device.
    Missing,
    /// The lower half of the device is busy; retry after a short delay.
    Busy,
    /// No computer is connected and listening.
    NotConnected,
    /// Another input or output error of the device.
    Io,
    /// The device answered the status command with an invalid payload.
    InvalidStatus,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Storage => "the configuration could not be read",
            Self::TooLarge => "the configuration file is too large",
            Self::Configuration => "the settings file is invalid",
            Self::Profiles => "the profiles file is invalid",
            Self::Missing => "the emulation device does not exist on this Xila",
            Self::Busy => "the emulation device is busy",
            Self::NotConnected => "no computer is connected",
            Self::Io => "the emulation device failed",
            Self::InvalidStatus => "the emulation device answered an invalid status",
        };
        formatter.write_str(text)
    }
}

impl core::error::Error for Error {}

/// Result of the Shortcuts application.
pub type Result<T> = core::result::Result<T, Error>;
