//! Keyboard layouts.
//!
//! HID sends key positions, not characters: a keyboard message holds the
//! usages of the physical keys. A shortcut written with a letter in the
//! configuration file, such as `Ctrl+Z`, means the key that shows Z on the
//! keyboard of the user, so the letter is mapped through the layout setting
//! to the usage of that physical key. The layout also gives the labels shown
//! on the buttons.

use crate::hid::keyboard;

/// HID usage of the semicolon key: the M position on AZERTY.
const SEMICOLON: u8 = 0x33;

/// A keyboard layout, chosen in the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// QWERTY, the HID usage order itself.
    #[default]
    Qwerty,
    /// AZERTY, French.
    Azerty,
    /// QWERTZ, German.
    Qwertz,
}

impl Layout {
    /// Every layout, in the order of the settings list.
    pub const ALL: [Self; 3] = [Self::Qwerty, Self::Azerty, Self::Qwertz];

    /// The identifier used in the settings file, such as `qwerty`.
    pub fn id(&self) -> &'static str {
        match self {
            Self::Qwerty => "qwerty",
            Self::Azerty => "azerty",
            Self::Qwertz => "qwertz",
        }
    }

    /// The layout of an identifier from the settings file.
    pub fn parse_id(text: &str) -> Option<Self> {
        Some(match text {
            "qwerty" => Self::Qwerty,
            "azerty" => Self::Azerty,
            "qwertz" => Self::Qwertz,
            _ => return None,
        })
    }

    /// The HID usage of the physical key that shows `label`, a letter or a
    /// digit. `None` when the label is not a letter or a digit.
    ///
    /// The digit row holds the same positions on all three layouts, so a
    /// digit always maps to its own usage. On AZERTY the digit row produces
    /// `&é"'(-è_çà` without Shift, so a shortcut that uses a digit gets
    /// Shift added to send the digit itself; see [`crate::shortcut`]. The
    /// letters differ where the layouts swap keys: AZERTY puts A, Z, Q, W
    /// and M on the QWERTY positions of Q, W, A, Z and semicolon, and
    /// QWERTZ swaps Y and Z.
    pub fn key_usage(&self, label: char) -> Option<u8> {
        let letter = label.to_ascii_lowercase();
        if !letter.is_ascii_alphabetic() {
            return match label {
                '1' => Some(keyboard::NUMBER_1),
                '2' => Some(keyboard::NUMBER_2),
                '3' => Some(keyboard::NUMBER_3),
                '4' => Some(keyboard::NUMBER_4),
                '5' => Some(keyboard::NUMBER_5),
                '6' => Some(keyboard::NUMBER_6),
                '7' => Some(keyboard::NUMBER_7),
                '8' => Some(keyboard::NUMBER_8),
                '9' => Some(keyboard::NUMBER_9),
                '0' => Some(keyboard::NUMBER_0),
                _ => None,
            };
        }
        let usage = match (self, letter) {
            (Self::Azerty, 'a') => keyboard::Q,
            (Self::Azerty, 'z') => keyboard::W,
            (Self::Azerty, 'q') => keyboard::A,
            (Self::Azerty, 'w') => keyboard::Z,
            (Self::Azerty, 'm') => SEMICOLON,
            (Self::Qwertz, 'y') => keyboard::Z,
            (Self::Qwertz, 'z') => keyboard::Y,
            _ => {
                let offset = letter as u8 - b'a';
                if offset >= 26 {
                    return None;
                }
                keyboard::A + offset
            }
        };
        Some(usage)
    }

    /// The label shown on the key of `usage`, used to display a shortcut in
    /// the layout of the user, such as `Ctrl+Z` on an AZERTY keyboard.
    pub fn key_label(&self, usage: u8) -> Option<char> {
        // On AZERTY the semicolon position carries the M key; on the other
        // layouts it has no letter or digit label.
        if usage == SEMICOLON {
            return match self {
                Self::Azerty => Some('m'),
                _ => None,
            };
        }
        let letter = match usage {
            keyboard::A..=keyboard::Z => (b'a' + (usage - keyboard::A)) as char,
            keyboard::NUMBER_1..=keyboard::NUMBER_9 => {
                (b'1' + (usage - keyboard::NUMBER_1)) as char
            }
            keyboard::NUMBER_0 => '0',
            _ => return None,
        };
        Some(match (self, usage, letter) {
            // On AZERTY the M position itself carries the comma.
            (Self::Azerty, keyboard::M, _) => ',',
            (Self::Azerty, _, 'a') => 'q',
            (Self::Azerty, _, 'z') => 'w',
            (Self::Azerty, _, 'q') => 'a',
            (Self::Azerty, _, 'w') => 'z',
            (Self::Qwertz, _, 'y') => 'z',
            (Self::Qwertz, _, 'z') => 'y',
            (_, _, letter) => letter,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_round_trip() {
        for layout in Layout::ALL {
            assert_eq!(Layout::parse_id(layout.id()), Some(layout));
        }
        assert_eq!(Layout::parse_id("dvorak"), None);
        assert_eq!(Layout::default(), Layout::Qwerty);
    }

    #[test]
    fn qwerty_maps_letters_to_their_own_usages() {
        let layout = Layout::Qwerty;
        for letter in 'a'..='z' {
            let usage = layout.key_usage(letter).unwrap();
            assert_eq!(usage, keyboard::A + (letter as u8 - b'a'));
            assert_eq!(layout.key_label(usage), Some(letter));
        }
        for digit in '0'..='9' {
            assert!(layout.key_usage(digit).is_some());
        }
        assert_eq!(layout.key_usage('4'), Some(keyboard::NUMBER_4));
        assert_eq!(layout.key_usage('0'), Some(keyboard::NUMBER_0));
        assert_eq!(layout.key_usage('+'), None);
    }

    #[test]
    fn azerty_maps_the_swapped_keys() {
        let layout = Layout::Azerty;
        // The key labeled A sits on the QWERTY Q position, and so on.
        assert_eq!(layout.key_usage('a'), Some(keyboard::Q));
        assert_eq!(layout.key_usage('z'), Some(keyboard::W));
        assert_eq!(layout.key_usage('q'), Some(keyboard::A));
        assert_eq!(layout.key_usage('w'), Some(keyboard::Z));
        assert_eq!(layout.key_usage('m'), Some(SEMICOLON));
        // The other letters keep their positions.
        assert_eq!(layout.key_usage('e'), Some(keyboard::E));
        assert_eq!(layout.key_usage('x'), Some(keyboard::X));
        // Labels go the other way: the QWERTY Q key shows A.
        assert_eq!(layout.key_label(keyboard::Q), Some('a'));
        assert_eq!(layout.key_label(keyboard::A), Some('q'));
        assert_eq!(layout.key_label(SEMICOLON), Some('m'));
        assert_eq!(layout.key_label(keyboard::E), Some('e'));
        // The M position carries the comma on AZERTY.
        assert_eq!(layout.key_label(keyboard::M), Some(','));
        // On QWERTY and QWERTZ the semicolon and M positions have no letter
        // or digit label.
        assert_eq!(Layout::Qwerty.key_label(SEMICOLON), None);
        assert_eq!(Layout::Qwerty.key_label(keyboard::M), Some('m'));
        assert_eq!(Layout::Qwertz.key_label(keyboard::M), Some('m'));
        // Ctrl+Z on AZERTY presses the QWERTY W key.
        assert_eq!(layout.key_usage('z'), Some(keyboard::W));
    }

    #[test]
    fn qwertz_swaps_y_and_z() {
        let layout = Layout::Qwertz;
        assert_eq!(layout.key_usage('z'), Some(keyboard::Y));
        assert_eq!(layout.key_usage('y'), Some(keyboard::Z));
        assert_eq!(layout.key_usage('a'), Some(keyboard::A));
        assert_eq!(layout.key_label(keyboard::Y), Some('z'));
        assert_eq!(layout.key_label(keyboard::Z), Some('y'));
    }

    #[test]
    fn key_labels_of_digits_are_the_digits() {
        for layout in Layout::ALL {
            assert_eq!(layout.key_label(keyboard::NUMBER_4), Some('4'));
            assert_eq!(layout.key_label(keyboard::NUMBER_0), Some('0'));
            assert_eq!(layout.key_label(keyboard::ENTER), None);
        }
    }
}
