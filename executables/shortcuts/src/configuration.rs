//! The settings file: the profile of the computer, the keyboard layout and
//! whether the mouse pad page shows. Every field is optional; the defaults
//! fit a Windows computer with a QWERTY keyboard.

use crate::error::{Error, Result};
use crate::layout::Layout;
use crate::profile::Profile;
use alloc::string::String;
use miniserde::{Deserialize, Serialize, json};

/// The settings of the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The profile of the computer the device is connected to.
    pub profile: Profile,
    /// The keyboard layout, for the labels and the letter keys of the
    /// shortcut texts.
    pub layout: Layout,
    /// Whether the mouse pad page shows.
    pub mouse_pad: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            profile: Profile::default(),
            layout: Layout::default(),
            mouse_pad: true,
        }
    }
}

/// The file layout of `settings.json`: every field is optional.
#[derive(Serialize, Deserialize)]
struct File {
    profile: Option<String>,
    layout: Option<String>,
    mouse_pad: Option<bool>,
}

impl Settings {
    /// Parses the settings file. An unknown profile or layout identifier is
    /// [`Error::Configuration`], so a typo does not silently change the
    /// computer.
    pub fn parse(text: &str) -> Result<Self> {
        let file: File = json::from_str(text).map_err(|_| Error::Configuration)?;
        let profile = match file.profile.as_deref() {
            None => Profile::default(),
            Some(text) => Profile::parse_id(text).ok_or(Error::Configuration)?,
        };
        let layout = match file.layout.as_deref() {
            None => Layout::default(),
            Some(text) => Layout::parse_id(text).ok_or(Error::Configuration)?,
        };
        Ok(Self {
            profile,
            layout,
            mouse_pad: file.mouse_pad.unwrap_or(true),
        })
    }

    /// Writes the settings as the settings file text.
    pub fn to_json(&self) -> String {
        json::to_string(&File {
            profile: Some(String::from(self.profile.id())),
            layout: Some(String::from(self.layout.id())),
            mouse_pad: Some(self.mouse_pad),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_windows_qwerty_with_mouse_pad() {
        assert_eq!(
            Settings::default(),
            Settings {
                profile: Profile::Windows,
                layout: Layout::Qwerty,
                mouse_pad: true,
            }
        );
        assert_eq!(Settings::parse("{}").unwrap(), Settings::default());
        assert!(!Settings::parse("").is_ok());
    }

    #[test]
    fn parses_every_field() {
        let settings =
            Settings::parse(r#"{"profile": "gnome", "layout": "azerty", "mouse_pad": false}"#)
                .unwrap();
        assert_eq!(
            settings,
            Settings {
                profile: Profile::Gnome,
                layout: Layout::Azerty,
                mouse_pad: false,
            }
        );
        // Round trip.
        assert_eq!(Settings::parse(&settings.to_json()).unwrap(), settings);
        // Partial files keep the defaults of the missing fields.
        let settings = Settings::parse(r#"{"layout": "qwertz"}"#).unwrap();
        assert_eq!(settings.profile, Profile::Windows);
        assert_eq!(settings.layout, Layout::Qwertz);
        assert!(settings.mouse_pad);
    }

    #[test]
    fn rejects_unknown_identifiers() {
        assert_eq!(
            Settings::parse(r#"{"profile": "os2"}"#),
            Err(Error::Configuration)
        );
        assert_eq!(
            Settings::parse(r#"{"layout": "dvorak"}"#),
            Err(Error::Configuration)
        );
        assert_eq!(Settings::parse("{"), Err(Error::Configuration));
        assert_eq!(
            Settings::parse(r#"{"mouse_pad": "yes"}"#),
            Err(Error::Configuration)
        );
    }
}
