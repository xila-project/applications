//! Profiles of the computer the device is connected to, and the shortcut
//! buttons they show.
//!
//! Each profile holds the buttons of one computer system: Windows, macOS or
//! Linux with GNOME. The built-in defaults are used while the profiles file
//! is absent; a user can write `profiles.json` in the application
//! configuration directory to replace the buttons of a profile and add their
//! own.

use crate::error::{Error, Result};
use crate::layout::Layout;
use crate::shortcut::{MAXIMUM_TEXT_LENGTH, Shortcut};
use alloc::string::String;
use alloc::vec::Vec;
use embedded_i18n::t;
use miniserde::{Deserialize, Serialize, json};

/// Most buttons in one profile.
pub const MAXIMUM_BUTTONS: usize = 24;
/// Longest button identifier kept.
pub const MAXIMUM_ID_LENGTH: usize = 32;
/// Longest custom button label kept.
pub const MAXIMUM_LABEL_LENGTH: usize = 48;

/// The computer systems with a built-in profile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Profile {
    /// Microsoft Windows.
    #[default]
    Windows,
    /// Apple macOS.
    Macos,
    /// Linux with the GNOME desktop.
    Gnome,
}

impl Profile {
    /// Every profile, in the order of the settings list.
    pub const ALL: [Self; 3] = [Self::Windows, Self::Macos, Self::Gnome];

    /// The identifier used in the settings and profiles files.
    pub fn id(&self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Macos => "macos",
            Self::Gnome => "gnome",
        }
    }

    /// The profile of an identifier from a file.
    pub fn parse_id(text: &str) -> Option<Self> {
        Some(match text {
            "windows" => Self::Windows,
            "macos" => Self::Macos,
            "gnome" => Self::Gnome,
            _ => return None,
        })
    }

    /// The built-in buttons of the profile, as shortcut texts.
    pub fn default_buttons(&self) -> Vec<Button> {
        let entries: &[(&str, &str)] = match self {
            Self::Windows => &[
                ("screenshot", "Win+Shift+S"),
                ("screenshot_full", "Print Screen"),
                ("copy", "Ctrl+C"),
                ("paste", "Ctrl+V"),
                ("undo", "Ctrl+Z"),
                ("lock", "Win+L"),
                ("switch_window", "Alt+Tab"),
                ("show_desktop", "Win+D"),
                // Windows 11 mutes the microphone system wide.
                ("mute_microphone", "Win+Alt+M"),
            ],
            Self::Macos => &[
                ("screenshot", "Cmd+Shift+4"),
                ("screenshot_full", "Cmd+Shift+3"),
                ("copy", "Cmd+C"),
                ("paste", "Cmd+V"),
                ("undo", "Cmd+Z"),
                ("lock", "Ctrl+Cmd+Q"),
                ("switch_window", "Cmd+Tab"),
                ("show_desktop", "Cmd+F3"),
            ],
            Self::Gnome => &[
                ("screenshot", "Print Screen"),
                ("screenshot_full", "Shift+Print Screen"),
                ("copy", "Ctrl+C"),
                ("paste", "Ctrl+V"),
                ("undo", "Ctrl+Z"),
                ("lock", "Super+L"),
                ("switch_window", "Alt+Tab"),
                ("show_desktop", "Super+D"),
            ],
        };
        entries
            .iter()
            .map(|(id, keys)| Button {
                id: String::from(*id),
                label: None,
                keys: String::from(*keys),
            })
            .collect()
    }
}

/// One button of a profile, as written in the profiles file. The `label` is
/// only for custom buttons: the built-in identifiers are translated. The
/// field is optional in the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub id: String,
    pub keys: String,
    pub label: Option<String>,
}

/// The profiles file: a list of profiles, each with its buttons. A profile
/// that is absent from the file keeps its built-in buttons.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Profiles {
    entries: Vec<(Profile, Vec<Button>)>,
}

/// The file layout of `profiles.json`.
#[derive(Serialize, Deserialize)]
struct File {
    profiles: Vec<ProfileFile>,
}

#[derive(Serialize, Deserialize)]
struct ProfileFile {
    id: String,
    buttons: Vec<Button>,
}

impl Profiles {
    /// Parses the profiles file. An unknown profile identifier, a duplicate
    /// button identifier or an over-long field is [`Error::Profiles`].
    pub fn parse(text: &str) -> Result<Self> {
        let file: File = json::from_str(text).map_err(|_| Error::Profiles)?;
        let mut entries = Vec::new();
        for profile_file in file.profiles {
            let Some(profile) = Profile::parse_id(&profile_file.id) else {
                return Err(Error::Profiles);
            };
            if entries.iter().any(|(known, _)| *known == profile) {
                return Err(Error::Profiles);
            }
            let mut buttons = Vec::new();
            for button in profile_file.buttons {
                if !valid_id(&button.id)
                    || button.keys.is_empty()
                    || button.keys.len() > MAXIMUM_TEXT_LENGTH
                    || button
                        .label
                        .as_ref()
                        .is_some_and(|label| label.is_empty() || label.len() > MAXIMUM_LABEL_LENGTH)
                    || buttons.iter().any(|known: &Button| known.id == button.id)
                {
                    return Err(Error::Profiles);
                }
                buttons.push(button);
            }
            if buttons.len() > MAXIMUM_BUTTONS {
                return Err(Error::Profiles);
            }
            entries.push((profile, buttons));
        }
        Ok(Self { entries })
    }

    /// Writes the profiles as the profiles file text.
    pub fn to_json(&self) -> String {
        json::to_string(&File {
            profiles: self
                .entries
                .iter()
                .map(|(profile, buttons)| ProfileFile {
                    id: String::from(profile.id()),
                    buttons: buttons.clone(),
                })
                .collect(),
        })
    }

    /// The buttons of a profile: the ones of the file when it has the
    /// profile, else the built-in defaults.
    pub fn buttons(&self, profile: Profile) -> Vec<Button> {
        self.entries
            .iter()
            .find(|(known, _)| *known == profile)
            .map(|(_, buttons)| buttons.clone())
            .unwrap_or_else(|| profile.default_buttons())
    }
}

/// A button with its parsed shortcut and the label to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledButton {
    pub id: String,
    pub label: String,
    pub shortcut: Shortcut,
}

/// Compiles the buttons of a profile with the keyboard layout: parses every
/// shortcut and resolves the label. A button whose shortcut does not parse
/// is skipped.
pub fn compile(buttons: &[Button], layout: Layout) -> Vec<CompiledButton> {
    let mut compiled = Vec::new();
    for button in buttons {
        let Ok(shortcut) = Shortcut::parse(&button.keys, layout) else {
            continue;
        };
        let label = label_of(button);
        compiled.push(CompiledButton {
            id: button.id.clone(),
            label,
            shortcut,
        });
    }
    compiled
}

/// The label of a button: the translation of a built-in identifier, else the
/// custom label of the file, else the shortcut text.
pub fn label_of(button: &Button) -> String {
    if let Some(label) = built_in_label(&button.id) {
        return label;
    }
    if let Some(label) = &button.label {
        return label.clone();
    }
    button.keys.clone()
}

/// The translated label of a built-in button identifier.
pub fn built_in_label(id: &str) -> Option<String> {
    Some(match id {
        "copy" => String::from(t!("buttons.copy")),
        "lock" => String::from(t!("buttons.lock")),
        "mute_microphone" => String::from(t!("buttons.mute_microphone")),
        "paste" => String::from(t!("buttons.paste")),
        "screenshot" => String::from(t!("buttons.screenshot")),
        "screenshot_full" => String::from(t!("buttons.screenshot_full")),
        "show_desktop" => String::from(t!("buttons.show_desktop")),
        "switch_window" => String::from(t!("buttons.switch_window")),
        "undo" => String::from(t!("buttons.undo")),
        _ => return None,
    })
}

/// A button identifier: one to [`MAXIMUM_ID_LENGTH`] letters, digits,
/// underscores or dashes, starting with a letter.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAXIMUM_ID_LENGTH
        && id
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase())
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// The error of a shortcut that does not parse, as a static text for tests.
#[cfg(test)]
pub fn parse_error_text(error: &crate::shortcut::ParseError) -> String {
    use crate::shortcut::ParseError;
    match error {
        ParseError::Empty => String::from("empty"),
        ParseError::Unknown(token) => token.clone(),
        ParseError::TooManyKeys => String::from("too many keys"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::keyboard;

    #[test]
    fn profile_identifiers_round_trip() {
        for profile in Profile::ALL {
            assert_eq!(Profile::parse_id(profile.id()), Some(profile));
        }
        assert_eq!(Profile::parse_id("os2"), None);
        assert_eq!(Profile::default(), Profile::Windows);
    }

    #[test]
    fn built_in_profiles_define_the_documented_shortcuts() {
        let windows = Profile::Windows.default_buttons();
        let find = |buttons: &[Button], id: &str| {
            buttons
                .iter()
                .find(|button| button.id == id)
                .unwrap_or_else(|| panic!("missing button {id}"))
                .keys
                .clone()
        };
        assert_eq!(find(&windows, "screenshot"), "Win+Shift+S");
        assert_eq!(find(&windows, "screenshot_full"), "Print Screen");
        assert_eq!(find(&windows, "copy"), "Ctrl+C");
        assert_eq!(find(&windows, "paste"), "Ctrl+V");
        assert_eq!(find(&windows, "undo"), "Ctrl+Z");
        assert_eq!(find(&windows, "lock"), "Win+L");
        assert_eq!(find(&windows, "switch_window"), "Alt+Tab");
        assert_eq!(find(&windows, "show_desktop"), "Win+D");
        assert_eq!(find(&windows, "mute_microphone"), "Win+Alt+M");

        let macos = Profile::Macos.default_buttons();
        assert_eq!(find(&macos, "screenshot"), "Cmd+Shift+4");
        assert_eq!(find(&macos, "screenshot_full"), "Cmd+Shift+3");
        assert_eq!(find(&macos, "lock"), "Ctrl+Cmd+Q");
        assert_eq!(find(&macos, "switch_window"), "Cmd+Tab");
        assert_eq!(find(&macos, "show_desktop"), "Cmd+F3");

        let gnome = Profile::Gnome.default_buttons();
        assert_eq!(find(&gnome, "screenshot"), "Print Screen");
        assert_eq!(find(&gnome, "screenshot_full"), "Shift+Print Screen");
        assert_eq!(find(&gnome, "lock"), "Super+L");
        assert_eq!(find(&gnome, "show_desktop"), "Super+D");
    }

    #[test]
    fn built_in_shortcuts_parse_on_every_layout() {
        for profile in Profile::ALL {
            for layout in Layout::ALL {
                for button in profile.default_buttons() {
                    assert!(
                        Shortcut::parse(&button.keys, layout).is_ok(),
                        "{} does not parse on {:?}",
                        button.keys,
                        layout
                    );
                }
            }
        }
    }

    #[test]
    fn profiles_file_round_trip_and_defaults() {
        // Without a file, every profile has its built-in buttons.
        let profiles = Profiles::default();
        assert_eq!(
            profiles.buttons(Profile::Windows),
            Profile::Windows.default_buttons()
        );

        // A file replaces the buttons of the profiles it lists.
        let text = r#"{
            "profiles": [
                {
                    "id": "macos",
                    "buttons": [
                        { "id": "screenshot", "keys": "Cmd+Shift+5" },
                        { "id": "my_tool", "keys": "Ctrl+Alt+T", "label": "My tool" }
                    ]
                }
            ]
        }"#;
        let profiles = Profiles::parse(text).unwrap();
        let buttons = profiles.buttons(Profile::Macos);
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0].keys, "Cmd+Shift+5");
        assert_eq!(buttons[1].label.as_deref(), Some("My tool"));
        assert_eq!(
            profiles.buttons(Profile::Gnome),
            Profile::Gnome.default_buttons()
        );
        assert_eq!(Profiles::parse(&profiles.to_json()).unwrap(), profiles);
    }

    #[test]
    fn profiles_file_rejects_invalid_content() {
        for text in [
            "{",
            r#"{"profiles": [{"id": "os2", "buttons": []}]}"#,
            r#"{"profiles": [{"id": "windows", "buttons": [{"id": "a", "keys": "Ctrl+C"}, {"id": "a", "keys": "Ctrl+V"}]}]}"#,
            r#"{"profiles": [{"id": "windows", "buttons": [{"id": "", "keys": "Ctrl+C"}]}]}"#,
            r#"{"profiles": [{"id": "windows", "buttons": [{"id": "a", "keys": ""}]}]}"#,
            r#"{"profiles": [{"id": "windows", "buttons": [{"id": "A", "keys": "Ctrl+C"}]}]}"#,
            r#"{"profiles": [{"id": "windows", "buttons": [{"id": "a", "keys": "Ctrl+C", "label": ""}]}]}"#,
            r#"{"profiles": [{"id": "windows", "buttons": []}, {"id": "windows", "buttons": []}]}"#,
        ] {
            assert_eq!(Profiles::parse(text), Err(Error::Profiles), "{text}");
        }
        // Too many buttons.
        let mut buttons = String::from("[");
        for index in 0..25 {
            if index > 0 {
                buttons.push(',');
            }
            buttons.push_str(&format!(r#"{{"id": "b{index}", "keys": "F{index}"}}"#));
        }
        buttons.push(']');
        let text = format!(r#"{{"profiles": [{{"id": "windows", "buttons": {buttons}}}]}}"#);
        assert_eq!(Profiles::parse(&text), Err(Error::Profiles));
    }

    #[test]
    fn compile_resolves_labels_and_parses_shortcuts() {
        let buttons = vec![
            Button {
                id: String::from("screenshot"),
                keys: String::from("Win+Shift+S"),
                label: None,
            },
            Button {
                id: String::from("my_tool"),
                keys: String::from("Ctrl+Alt+T"),
                label: Some(String::from("My tool")),
            },
            Button {
                id: String::from("broken"),
                keys: String::from("Ctrl+Nope"),
                label: None,
            },
        ];
        let compiled = compile(&buttons, Layout::Qwerty);
        assert_eq!(compiled.len(), 2);
        assert_eq!(compiled[0].label, built_in_label("screenshot").unwrap());
        assert_eq!(compiled[0].shortcut.keys[0], keyboard::S);
        assert_eq!(compiled[1].label, "My tool");
        assert_eq!(compiled[1].shortcut.keys[0], keyboard::T);
        // A button without a custom label falls back to its shortcut text.
        let fallback = Button {
            id: String::from("other"),
            keys: String::from("F9"),
            label: None,
        };
        assert_eq!(label_of(&fallback), "F9");
    }
}
