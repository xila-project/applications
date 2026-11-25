//! Shortcut parsing.
//!
//! A shortcut is written in the configuration file as tokens joined by `+`,
//! for example `Win+Shift+S`, `Ctrl+C` or `Print Screen`. The tokens before
//! the last key are modifiers; the key itself is a letter, a digit or a
//! named key. Letters map through the keyboard layout setting, because HID
//! sends key positions: `Ctrl+Z` means the key that shows Z on the keyboard
//! of the user.

use crate::channel::{self, Channel};
use crate::error::Result;
use crate::hid::{KEY_COUNT, Modifiers, keyboard};
use crate::layout::Layout;
use alloc::string::String;
use alloc::vec::Vec;

/// Longest shortcut text accepted, such as `Ctrl+Alt+Shift+Page Down`.
pub const MAXIMUM_TEXT_LENGTH: usize = 64;

/// What can be wrong in a shortcut text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// The text is empty or only spaces.
    Empty,
    /// A token names no known modifier or key.
    Unknown(String),
    /// More key usages than one keyboard message holds.
    TooManyKeys,
}

/// A parsed shortcut: the modifier bitmask and the key usages to press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub modifiers: Modifiers,
    pub keys: [u8; KEY_COUNT],
    /// The text as written in the configuration file, shown on the button.
    pub text: String,
}

impl Shortcut {
    /// Parses a shortcut text with the given keyboard layout. The text must
    /// not be longer than [`MAXIMUM_TEXT_LENGTH`]; the profiles check that
    /// before parsing.
    pub fn parse(text: &str, layout: Layout) -> core::result::Result<Self, ParseError> {
        let mut modifiers = Modifiers::NONE;
        let mut keys = [0u8; KEY_COUNT];
        let mut key_count = 0usize;
        let mut saw_any = false;
        for token in text.split('+') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            saw_any = true;
            if let Some(bit) = modifier_of(token) {
                modifiers.insert(bit);
                continue;
            }
            let usage =
                key_of(token, layout).ok_or_else(|| ParseError::Unknown(String::from(token)))?;
            if key_count >= KEY_COUNT {
                return Err(ParseError::TooManyKeys);
            }
            keys[key_count] = usage;
            key_count += 1;
        }
        if !saw_any {
            return Err(ParseError::Empty);
        }
        Ok(Self {
            modifiers,
            keys,
            text: String::from(text.trim()),
        })
    }

    /// Presses and releases this shortcut on the device.
    pub fn tap(&self, channel: &mut impl Channel) -> Result<()> {
        channel::tap_keys(channel, self.modifiers, self.keys)
    }

    /// The text of the shortcut with the key labels of the layout, such as
    /// `Ctrl+Z` shown on an AZERTY keyboard.
    pub fn display_text(&self, layout: Layout) -> String {
        let mut parts = Vec::new();
        let modifiers = self.modifiers;
        if modifiers.contains(Modifiers::LEFT_GUI) {
            parts.push(String::from("Win"));
        }
        if modifiers.contains(Modifiers::LEFT_CONTROL) {
            parts.push(String::from("Ctrl"));
        }
        if modifiers.contains(Modifiers::LEFT_SHIFT) {
            parts.push(String::from("Shift"));
        }
        if modifiers.contains(Modifiers::LEFT_ALT) {
            parts.push(String::from("Alt"));
        }
        if modifiers.contains(Modifiers::RIGHT_CONTROL) {
            parts.push(String::from("Right Ctrl"));
        }
        if modifiers.contains(Modifiers::RIGHT_SHIFT) {
            parts.push(String::from("Right Shift"));
        }
        if modifiers.contains(Modifiers::RIGHT_ALT) {
            parts.push(String::from("Alt Gr"));
        }
        if modifiers.contains(Modifiers::RIGHT_GUI) {
            parts.push(String::from("Right Win"));
        }
        for &usage in &self.keys {
            if usage == 0 {
                continue;
            }
            if let Some(label) = layout.key_label(usage) {
                parts.push(String::from(label.to_ascii_uppercase()));
            } else if let Some(name) = key_name(usage) {
                parts.push(String::from(name));
            }
        }
        parts.join("+")
    }
}

/// The modifier bitmask of a modifier token, case insensitive. `None` when
/// the token is not a modifier.
fn modifier_of(token: &str) -> Option<Modifiers> {
    Some(match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "lctrl" | "left ctrl" | "left control" => Modifiers::LEFT_CONTROL,
        "rctrl" | "right ctrl" | "right control" => Modifiers::RIGHT_CONTROL,
        "shift" | "lshift" | "left shift" => Modifiers::LEFT_SHIFT,
        "rshift" | "right shift" => Modifiers::RIGHT_SHIFT,
        "alt" | "option" | "lalt" | "left alt" | "left option" => Modifiers::LEFT_ALT,
        "altgr" | "alt gr" | "ralt" | "right alt" | "right alt gr" => Modifiers::RIGHT_ALT,
        "win" | "windows" | "super" | "cmd" | "command" | "gui" | "meta" | "left gui"
        | "left win" | "left super" | "left cmd" => Modifiers::LEFT_GUI,
        "rgui" | "right gui" | "right win" | "right super" | "right cmd" => Modifiers::RIGHT_GUI,
        _ => return None,
    })
}

/// The HID usage of a key token, case insensitive. `None` when the token
/// names no key.
fn key_of(token: &str, layout: Layout) -> Option<u8> {
    let text = token.to_ascii_lowercase();
    if text.len() == 1
        && let Some(character) = text.chars().next()
        && let Some(usage) = layout.key_usage(character)
    {
        return Some(usage);
    }
    if let Some(number) = text.strip_prefix('f')
        && let Ok(number) = number.parse::<u8>()
    {
        // F1 to F12 are contiguous, F13 to F24 follow after a gap.
        return match number {
            1..=12 => Some(keyboard::F1 + number - 1),
            13..=24 => Some(keyboard::F13 + number - 13),
            _ => None,
        };
    }
    Some(match text.as_str() {
        "enter" | "return" => keyboard::ENTER,
        "escape" | "esc" => keyboard::ESCAPE,
        "backspace" => keyboard::BACKSPACE,
        "tab" => keyboard::TAB,
        "space" | "spacebar" | "space bar" => keyboard::SPACE,
        "caps lock" | "capslock" => keyboard::CAPS_LOCK,
        "print screen" | "printscreen" | "prtsc" | "print scr" => keyboard::PRINT_SCREEN,
        "scroll lock" | "scrolllock" => keyboard::SCROLL_LOCK,
        "pause" | "break" => keyboard::PAUSE,
        "insert" | "ins" => keyboard::INSERT,
        "home" => keyboard::HOME,
        "end" => keyboard::END,
        "page up" | "pageup" | "pgup" => keyboard::PAGE_UP,
        "page down" | "pagedown" | "pgdn" => keyboard::PAGE_DOWN,
        "delete" | "del" => keyboard::DELETE,
        "left" | "arrow left" => keyboard::ARROW_LEFT,
        "right" | "arrow right" => keyboard::ARROW_RIGHT,
        "up" | "arrow up" => keyboard::ARROW_UP,
        "down" | "arrow down" => keyboard::ARROW_DOWN,
        _ => return None,
    })
}

/// The display name of a key usage, used by [`Shortcut::display_text`].
fn key_name(usage: u8) -> Option<&'static str> {
    Some(match usage {
        keyboard::ENTER => "Enter",
        keyboard::ESCAPE => "Escape",
        keyboard::BACKSPACE => "Backspace",
        keyboard::TAB => "Tab",
        keyboard::SPACE => "Space",
        keyboard::CAPS_LOCK => "Caps Lock",
        keyboard::PRINT_SCREEN => "Print Screen",
        keyboard::SCROLL_LOCK => "Scroll Lock",
        keyboard::PAUSE => "Pause",
        keyboard::INSERT => "Insert",
        keyboard::HOME => "Home",
        keyboard::END => "End",
        keyboard::PAGE_UP => "Page Up",
        keyboard::PAGE_DOWN => "Page Down",
        keyboard::DELETE => "Delete",
        keyboard::ARROW_LEFT => "Left",
        keyboard::ARROW_RIGHT => "Right",
        keyboard::ARROW_UP => "Up",
        keyboard::ARROW_DOWN => "Down",
        _ => {
            if (keyboard::F1..=keyboard::F24).contains(&usage) {
                // Function keys are shown as F1 to F24; the text is short
                // enough to build without formatting macros.
                return Some(match usage - keyboard::F1 + 1 {
                    1 => "F1",
                    2 => "F2",
                    3 => "F3",
                    4 => "F4",
                    5 => "F5",
                    6 => "F6",
                    7 => "F7",
                    8 => "F8",
                    9 => "F9",
                    10 => "F10",
                    11 => "F11",
                    12 => "F12",
                    13 => "F13",
                    14 => "F14",
                    15 => "F15",
                    16 => "F16",
                    17 => "F17",
                    18 => "F18",
                    19 => "F19",
                    20 => "F20",
                    21 => "F21",
                    22 => "F22",
                    23 => "F23",
                    _ => "F24",
                });
            }
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> core::result::Result<Shortcut, ParseError> {
        Shortcut::parse(text, Layout::Qwerty)
    }

    #[test]
    fn parses_modifiers_and_a_letter_key() {
        let shortcut = parse("Ctrl+C").unwrap();
        assert_eq!(shortcut.modifiers, Modifiers::LEFT_CONTROL);
        assert_eq!(shortcut.keys, [keyboard::C, 0, 0, 0, 0, 0]);
        assert_eq!(shortcut.text, "Ctrl+C");

        let shortcut = parse("Win+Shift+S").unwrap();
        assert_eq!(shortcut.modifiers.bits(), 0x0A);
        assert_eq!(shortcut.keys[0], keyboard::S);

        let shortcut = parse("ctrl+alt+delete").unwrap();
        assert_eq!(shortcut.modifiers.bits(), 0x05);
        assert_eq!(shortcut.keys[0], keyboard::DELETE);

        let shortcut = parse("Cmd+Shift+4").unwrap();
        assert_eq!(
            shortcut.modifiers,
            Modifiers::LEFT_GUI | Modifiers::LEFT_SHIFT
        );
        assert_eq!(shortcut.keys[0], keyboard::NUMBER_4);
    }

    #[test]
    fn parses_named_keys() {
        assert_eq!(
            parse("Print Screen").unwrap().keys[0],
            keyboard::PRINT_SCREEN
        );
        assert_eq!(
            parse("Shift+Print Screen").unwrap().keys[0],
            keyboard::PRINT_SCREEN
        );
        assert_eq!(parse("Super+L").unwrap().keys[0], keyboard::L);
        assert_eq!(parse("Ctrl+Cmd+Q").unwrap().modifiers.bits(), 0x09);
        assert_eq!(parse("Alt+Tab").unwrap().keys[0], keyboard::TAB);
        assert_eq!(parse("Win+D").unwrap().keys[0], keyboard::D);
        assert_eq!(parse("Page Up").unwrap().keys[0], keyboard::PAGE_UP);
        assert_eq!(parse("F5").unwrap().keys[0], keyboard::F5);
        assert_eq!(parse("Ctrl+F24").unwrap().keys[0], keyboard::F24);
        assert_eq!(
            parse("AltGr+Space").unwrap().modifiers,
            Modifiers::RIGHT_ALT
        );
    }

    #[test]
    fn letters_follow_the_layout_setting() {
        // Ctrl+Z means the key labeled Z: on QWERTY it is the Z usage, on
        // AZERTY the W usage, on QWERTZ the Y usage.
        assert_eq!(
            Shortcut::parse("Ctrl+Z", Layout::Qwerty).unwrap().keys[0],
            keyboard::Z
        );
        assert_eq!(
            Shortcut::parse("Ctrl+Z", Layout::Azerty).unwrap().keys[0],
            keyboard::W
        );
        assert_eq!(
            Shortcut::parse("Ctrl+Z", Layout::Qwertz).unwrap().keys[0],
            keyboard::Y
        );
        // Ctrl+A on AZERTY presses the QWERTY Q key.
        assert_eq!(
            Shortcut::parse("Ctrl+A", Layout::Azerty).unwrap().keys[0],
            keyboard::Q
        );
        // Digits do not move between layouts.
        assert_eq!(
            Shortcut::parse("Cmd+Shift+4", Layout::Azerty).unwrap().keys[0],
            keyboard::NUMBER_4
        );
    }

    #[test]
    fn rejects_invalid_texts() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("   "), Err(ParseError::Empty));
        assert_eq!(
            parse("Ctrl+Hyper"),
            Err(ParseError::Unknown(String::from("Hyper")))
        );
        assert_eq!(parse("A+B+C+D+E+F+G"), Err(ParseError::TooManyKeys));
        // A shortcut without a key is accepted as a modifier-only press.
        let shortcut = parse("Ctrl").unwrap();
        assert_eq!(shortcut.modifiers, Modifiers::LEFT_CONTROL);
        assert_eq!(shortcut.keys, [0; KEY_COUNT]);
    }

    #[test]
    fn display_text_shows_the_layout_labels() {
        let shortcut = Shortcut::parse("Ctrl+Z", Layout::Qwerty).unwrap();
        assert_eq!(shortcut.display_text(Layout::Qwerty), "Ctrl+Z");
        assert_eq!(shortcut.display_text(Layout::Azerty), "Ctrl+W");
        let shortcut = Shortcut::parse("Win+Shift+S", Layout::Qwerty).unwrap();
        assert_eq!(shortcut.display_text(Layout::Qwerty), "Win+Shift+S");
        let shortcut = Shortcut::parse("Print Screen", Layout::Qwerty).unwrap();
        assert_eq!(shortcut.display_text(Layout::Qwerty), "Print Screen");
    }

    #[test]
    fn tap_sends_press_and_release() {
        struct Recorder {
            bytes: Vec<u8>,
        }
        impl Channel for Recorder {
            fn write(&mut self, bytes: &[u8]) -> Result<usize> {
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn control(&mut self, _command: u32, _input: &[u8], _output: &mut [u8]) -> Result<()> {
                Ok(())
            }
        }
        let mut recorder = Recorder { bytes: Vec::new() };
        let shortcut = parse("Ctrl+Shift+S").unwrap();
        shortcut.tap(&mut recorder).unwrap();
        assert_eq!(
            recorder.bytes,
            [
                vec![1, 0x03, keyboard::S, 0, 0, 0, 0, 0],
                vec![1, 0, 0, 0, 0, 0, 0, 0],
            ]
            .concat()
        );
    }
}
