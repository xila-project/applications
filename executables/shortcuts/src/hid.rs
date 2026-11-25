//! Mirror of the Core `device::human_interface` contract, the emulation
//! device node `/devices/human_interface/emulation`.
//!
//! The WebAssembly guest cannot depend on the Core `device` crate at run
//! time, so this module mirrors the wire format: the 8-byte messages, the
//! usage constants and the `GET_STATUS` control identifier. The test
//! `mirror_matches_the_core_device_crate` compares every constant with the
//! Core crate, which is a development dependency, so the two cannot drift.

use core::mem::size_of;
use core::ops::{BitAnd, BitOr};

/// Path of the emulation device node in the virtual file system.
pub const EMULATION_PATH: &str = "/devices/human_interface/emulation";

/// Size of one message in bytes.
pub const MESSAGE_SIZE: usize = 8;

/// Kind byte of a keyboard message.
pub const KIND_KEYBOARD: u8 = 1;
/// Kind byte of a consumer control message.
pub const KIND_CONSUMER: u8 = 2;
/// Kind byte of a mouse message.
pub const KIND_MOUSE: u8 = 3;
/// Kind byte of a system control message.
pub const KIND_SYSTEM: u8 = 4;

/// Number of key usages in one keyboard message.
pub const KEY_COUNT: usize = 6;

/// Version of every control ABI struct in this module.
pub const ABI_VERSION: u16 = 1;

/// Keyboard indicator bit 0: num lock is on.
pub const INDICATOR_NUM_LOCK: u8 = 1 << 0;
/// Keyboard indicator bit 1: caps lock is on.
pub const INDICATOR_CAPS_LOCK: u8 = 1 << 1;
/// Keyboard indicator bit 2: scroll lock is on.
pub const INDICATOR_SCROLL_LOCK: u8 = 1 << 2;

/// Kind byte of the human interface control commands (`b'h'`).
pub const KIND_HUMAN_INTERFACE: u8 = b'h';

/// Builds a control command identifier the way `file_system` does:
/// direction in the top two bits, payload size in the next fourteen, then
/// the kind byte and the command number.
const fn control_identifier(direction: u32, payload_size: u32, number: u8) -> u32 {
    direction | (payload_size << 16) | ((KIND_HUMAN_INTERFACE as u32) << 8) | number as u32
}

/// Input of [`GET_STATUS`]: the ABI version and a reserved field.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VersionAbi {
    pub version: u16,
    pub reserved: u16,
}

/// Output of [`GET_STATUS`]: the connection state and the keyboard indicators.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusAbi {
    pub version: u16,
    /// `1` when a computer is connected and listening, else `0`.
    pub connected: u8,
    /// Keyboard indicator bitmask from the computer.
    pub indicators: u8,
    pub reserved: [u8; 4],
}

/// Mirrors `file_system`'s `ControlCommandInputOutput<VersionAbi, StatusAbi>`:
/// the payload size field of a control identifier is the size of the input
/// and output structs side by side.
struct CommandPayload {
    _input: VersionAbi,
    _output: StatusAbi,
}

/// `GET_STATUS` control command: reads the connection state and the
/// keyboard indicators of the computer.
pub const GET_STATUS: u32 = control_identifier(1 << 30, size_of::<CommandPayload>() as u32, 0x00);

/// Keyboard modifier bitmask, HID standard order: left control, left shift,
/// left alt, left GUI, then the right ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    /// No bit set.
    pub const NONE: Self = Self(0);
    /// Left control.
    pub const LEFT_CONTROL: Self = Self(1 << 0);
    /// Left shift.
    pub const LEFT_SHIFT: Self = Self(1 << 1);
    /// Left alt.
    pub const LEFT_ALT: Self = Self(1 << 2);
    /// Left GUI (Windows, command, super).
    pub const LEFT_GUI: Self = Self(1 << 3);
    /// Right control.
    pub const RIGHT_CONTROL: Self = Self(1 << 4);
    /// Right shift.
    pub const RIGHT_SHIFT: Self = Self(1 << 5);
    /// Right alt (alt gr).
    pub const RIGHT_ALT: Self = Self(1 << 6);
    /// Right GUI.
    pub const RIGHT_GUI: Self = Self(1 << 7);
    /// Every defined bit.
    pub const ALL: Self = Self(0xFF);

    /// Empty bitmask.
    pub const fn empty() -> Self {
        Self::NONE
    }

    /// Bitmask from raw bits; unknown bits are retained.
    pub const fn from_bits_retain(bits: u8) -> Self {
        Self(bits)
    }

    /// Raw bits of the bitmask.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// `true` when no bit is set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `true` when every bit of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Union of two bitmasks.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Set the bits of `other`.
    pub const fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl BitOr for Modifiers {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

impl BitAnd for Modifiers {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Mouse buttons bitmask: left, right, middle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MouseButtons(u8);

impl MouseButtons {
    /// No bit set.
    pub const NONE: Self = Self(0);
    /// Left button.
    pub const LEFT: Self = Self(1 << 0);
    /// Right button.
    pub const RIGHT: Self = Self(1 << 1);
    /// Middle button.
    pub const MIDDLE: Self = Self(1 << 2);
    /// Every defined bit.
    pub const ALL: Self = Self(0x07);

    /// Empty bitmask.
    pub const fn empty() -> Self {
        Self::NONE
    }

    /// Bitmask from raw bits; unknown bits are retained.
    pub const fn from_bits_retain(bits: u8) -> Self {
        Self(bits)
    }

    /// Raw bits of the bitmask.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

impl BitOr for MouseButtons {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// One message of the emulation device.
///
/// A message is the full state of one function: a keyboard message lists
/// every pressed key, so an all-zero keyboard message releases all keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Message {
    /// Keyboard state: modifier bitmask and up to six key usages from the
    /// HID Keyboard/Keypad page (0x07). `0` marks an empty slot.
    Keyboard {
        /// Modifier bitmask, see [`Modifiers`].
        modifiers: Modifiers,
        /// Up to six pressed key usages, `0` = none.
        keys: [u8; KEY_COUNT],
    },
    /// Consumer control usage from the HID Consumer page (0x0C). `0`
    /// releases the control.
    Consumer {
        /// Consumer usage, little endian on the wire.
        usage: u16,
    },
    /// Mouse movement and buttons.
    Mouse {
        /// Buttons bitmask, see [`MouseButtons`].
        buttons: MouseButtons,
        /// Horizontal movement in pixels, positive to the right.
        x: i8,
        /// Vertical movement in pixels, positive down.
        y: i8,
        /// Vertical wheel steps, positive up.
        wheel: i8,
        /// Horizontal pan steps, positive right.
        pan: i8,
    },
    /// System control usage from the HID Generic Desktop page. `0`
    /// releases the control.
    System {
        /// System usage.
        usage: u8,
    },
}

impl Message {
    /// Encode the message as its 8 wire bytes, kind in byte 0. Reserved
    /// bytes are zero.
    pub fn encode(&self) -> [u8; MESSAGE_SIZE] {
        let mut bytes = [0u8; MESSAGE_SIZE];
        match self {
            Self::Keyboard { modifiers, keys } => {
                bytes[0] = KIND_KEYBOARD;
                bytes[1] = modifiers.bits();
                bytes[2..].copy_from_slice(keys);
            }
            Self::Consumer { usage } => {
                bytes[0] = KIND_CONSUMER;
                bytes[1..3].copy_from_slice(&usage.to_le_bytes());
            }
            Self::Mouse {
                buttons,
                x,
                y,
                wheel,
                pan,
            } => {
                bytes[0] = KIND_MOUSE;
                bytes[1] = buttons.bits();
                bytes[2] = *x as u8;
                bytes[3] = *y as u8;
                bytes[4] = *wheel as u8;
                bytes[5] = *pan as u8;
            }
            Self::System { usage } => {
                bytes[0] = KIND_SYSTEM;
                bytes[1] = *usage;
            }
        }
        bytes
    }
}

/// Keyboard/Keypad page (0x07) usages.
pub mod keyboard {
    pub const A: u8 = 0x04;
    pub const B: u8 = 0x05;
    pub const C: u8 = 0x06;
    pub const D: u8 = 0x07;
    pub const E: u8 = 0x08;
    pub const F: u8 = 0x09;
    pub const G: u8 = 0x0A;
    pub const H: u8 = 0x0B;
    pub const I: u8 = 0x0C;
    pub const J: u8 = 0x0D;
    pub const K: u8 = 0x0E;
    pub const L: u8 = 0x0F;
    pub const M: u8 = 0x10;
    pub const N: u8 = 0x11;
    pub const O: u8 = 0x12;
    pub const P: u8 = 0x13;
    pub const Q: u8 = 0x14;
    pub const R: u8 = 0x15;
    pub const S: u8 = 0x16;
    pub const T: u8 = 0x17;
    pub const U: u8 = 0x18;
    pub const V: u8 = 0x19;
    pub const W: u8 = 0x1A;
    pub const X: u8 = 0x1B;
    pub const Y: u8 = 0x1C;
    pub const Z: u8 = 0x1D;
    pub const NUMBER_1: u8 = 0x1E;
    pub const NUMBER_2: u8 = 0x1F;
    pub const NUMBER_3: u8 = 0x20;
    pub const NUMBER_4: u8 = 0x21;
    pub const NUMBER_5: u8 = 0x22;
    pub const NUMBER_6: u8 = 0x23;
    pub const NUMBER_7: u8 = 0x24;
    pub const NUMBER_8: u8 = 0x25;
    pub const NUMBER_9: u8 = 0x26;
    pub const NUMBER_0: u8 = 0x27;
    pub const ENTER: u8 = 0x28;
    pub const ESCAPE: u8 = 0x29;
    pub const BACKSPACE: u8 = 0x2A;
    pub const TAB: u8 = 0x2B;
    pub const SPACE: u8 = 0x2C;
    pub const CAPS_LOCK: u8 = 0x39;
    pub const F1: u8 = 0x3A;
    pub const F2: u8 = 0x3B;
    pub const F3: u8 = 0x3C;
    pub const F4: u8 = 0x3D;
    pub const F5: u8 = 0x3E;
    pub const F6: u8 = 0x3F;
    pub const F7: u8 = 0x40;
    pub const F8: u8 = 0x41;
    pub const F9: u8 = 0x42;
    pub const F10: u8 = 0x43;
    pub const F11: u8 = 0x44;
    pub const F12: u8 = 0x45;
    pub const PRINT_SCREEN: u8 = 0x46;
    pub const SCROLL_LOCK: u8 = 0x47;
    pub const PAUSE: u8 = 0x48;
    pub const INSERT: u8 = 0x49;
    pub const HOME: u8 = 0x4A;
    pub const PAGE_UP: u8 = 0x4B;
    pub const DELETE: u8 = 0x4C;
    pub const END: u8 = 0x4D;
    pub const PAGE_DOWN: u8 = 0x4E;
    pub const ARROW_RIGHT: u8 = 0x4F;
    pub const ARROW_LEFT: u8 = 0x50;
    pub const ARROW_DOWN: u8 = 0x51;
    pub const ARROW_UP: u8 = 0x52;
    pub const F13: u8 = 0x68;
    pub const F14: u8 = 0x69;
    pub const F15: u8 = 0x6A;
    pub const F16: u8 = 0x6B;
    pub const F17: u8 = 0x6C;
    pub const F18: u8 = 0x6D;
    pub const F19: u8 = 0x6E;
    pub const F20: u8 = 0x6F;
    pub const F21: u8 = 0x70;
    pub const F22: u8 = 0x71;
    pub const F23: u8 = 0x72;
    pub const F24: u8 = 0x73;
}

/// Consumer page (0x0C) usages: media remote controls.
pub mod consumer {
    /// Play/pause.
    pub const PLAY_PAUSE: u16 = 0x00CD;
    /// Next track.
    pub const NEXT_TRACK: u16 = 0x00B5;
    /// Previous track.
    pub const PREVIOUS_TRACK: u16 = 0x00B6;
    /// Stop.
    pub const STOP: u16 = 0x00B7;
    /// Volume up.
    pub const VOLUME_UP: u16 = 0x00E9;
    /// Volume down.
    pub const VOLUME_DOWN: u16 = 0x00EA;
    /// Mute.
    pub const MUTE: u16 = 0x00E2;
}

/// Generic Desktop page (0x01) system control usages.
pub mod system {
    /// System power down.
    pub const POWER_DOWN: u8 = 0x81;
    /// System sleep.
    pub const SLEEP: u8 = 0x82;
    /// System wake up.
    pub const WAKE_UP: u8 = 0x83;
}

/// Wire bytes of [`VersionAbi`], little endian as the C layout sends them.
pub fn encode_version_abi(abi: &VersionAbi) -> [u8; 4] {
    let mut bytes = [0u8; 4];
    bytes[..2].copy_from_slice(&abi.version.to_le_bytes());
    bytes[2..].copy_from_slice(&abi.reserved.to_le_bytes());
    bytes
}

/// Reads a [`StatusAbi`] from its 8 wire bytes. A wrong length, version or
/// non-zero reserved field is [`crate::Error::InvalidStatus`].
pub fn decode_status_abi(bytes: &[u8]) -> crate::Result<StatusAbi> {
    if bytes.len() != size_of::<StatusAbi>() {
        return Err(crate::Error::InvalidStatus);
    }
    let abi = StatusAbi {
        version: u16::from_le_bytes([bytes[0], bytes[1]]),
        connected: bytes[2],
        indicators: bytes[3],
        reserved: [bytes[4], bytes[5], bytes[6], bytes[7]],
    };
    if abi.version != ABI_VERSION || abi.reserved != [0; 4] {
        return Err(crate::Error::InvalidStatus);
    }
    Ok(abi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_the_documented_wire_layout() {
        let keyboard = Message::Keyboard {
            modifiers: Modifiers::LEFT_CONTROL | Modifiers::LEFT_SHIFT,
            keys: [keyboard::A, keyboard::ENTER, 0, 0, 0, 0],
        };
        assert_eq!(
            keyboard.encode(),
            [
                KIND_KEYBOARD,
                0b0000_0011,
                keyboard::A,
                keyboard::ENTER,
                0,
                0,
                0,
                0
            ]
        );
        assert_eq!(
            Message::Keyboard {
                modifiers: Modifiers::NONE,
                keys: [0; KEY_COUNT],
            }
            .encode(),
            [KIND_KEYBOARD, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            Message::Consumer {
                usage: consumer::PLAY_PAUSE
            }
            .encode(),
            [KIND_CONSUMER, 0xCD, 0x00, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            Message::Consumer { usage: 0 }.encode(),
            [KIND_CONSUMER, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            Message::Mouse {
                buttons: MouseButtons::LEFT | MouseButtons::RIGHT,
                x: -12,
                y: 34,
                wheel: -1,
                pan: 2,
            }
            .encode(),
            [KIND_MOUSE, 0b0000_0011, 0xF4, 0x22, 0xFF, 0x02, 0, 0]
        );
        assert_eq!(
            Message::System {
                usage: system::WAKE_UP
            }
            .encode(),
            [KIND_SYSTEM, 0x83, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn modifiers_use_the_hid_standard_bit_order() {
        assert_eq!(Modifiers::LEFT_CONTROL.bits(), 0x01);
        assert_eq!(Modifiers::LEFT_SHIFT.bits(), 0x02);
        assert_eq!(Modifiers::LEFT_ALT.bits(), 0x04);
        assert_eq!(Modifiers::LEFT_GUI.bits(), 0x08);
        assert_eq!(Modifiers::RIGHT_CONTROL.bits(), 0x10);
        assert_eq!(Modifiers::RIGHT_SHIFT.bits(), 0x20);
        assert_eq!(Modifiers::RIGHT_ALT.bits(), 0x40);
        assert_eq!(Modifiers::RIGHT_GUI.bits(), 0x80);
        assert_eq!(Modifiers::ALL.bits(), 0xFF);
        let shortcut = Modifiers::LEFT_CONTROL | Modifiers::LEFT_SHIFT;
        assert_eq!(shortcut.bits(), 0x03);
        assert!(!shortcut.is_empty());
        assert!(Modifiers::NONE.is_empty());
        let mut pressed = Modifiers::empty();
        pressed.insert(Modifiers::RIGHT_GUI);
        assert_eq!(pressed, Modifiers::RIGHT_GUI);
        assert_eq!((pressed & Modifiers::ALL).bits(), 0x80);
    }

    #[test]
    fn status_abi_round_trips_through_its_wire_bytes() {
        let abi = VersionAbi {
            version: ABI_VERSION,
            reserved: 0,
        };
        assert_eq!(encode_version_abi(&abi), [1, 0, 0, 0]);
        let status = StatusAbi {
            version: ABI_VERSION,
            connected: 1,
            indicators: INDICATOR_CAPS_LOCK,
            reserved: [0; 4],
        };
        let mut bytes = [0u8; 8];
        bytes[..2].copy_from_slice(&status.version.to_le_bytes());
        bytes[2] = status.connected;
        bytes[3] = status.indicators;
        assert_eq!(decode_status_abi(&bytes), Ok(status));
        // A wrong version or a non-zero reserved byte is refused.
        bytes[0] = 2;
        assert_eq!(decode_status_abi(&bytes), Err(crate::Error::InvalidStatus));
        bytes[0] = 1;
        bytes[7] = 0xFF;
        assert_eq!(decode_status_abi(&bytes), Err(crate::Error::InvalidStatus));
        assert_eq!(
            decode_status_abi(&bytes[..7]),
            Err(crate::Error::InvalidStatus)
        );
    }

    #[test]
    fn get_status_identifier_packs_direction_size_kind_and_number() {
        // Read direction, a 12-byte payload (VersionAbi and StatusAbi side
        // by side), kind b'h', number 0.
        assert_eq!(GET_STATUS, (1 << 30) | (12 << 16) | (0x68 << 8));
        assert_eq!(size_of::<VersionAbi>(), 4);
        assert_eq!(size_of::<StatusAbi>(), 8);
    }

    /// Every mirrored constant is compared with the Core `device` crate,
    /// which is a development dependency of this crate.
    #[test]
    fn mirror_matches_the_core_device_crate() {
        use device::human_interface as core;
        use file_system::ControlCommand;

        assert_eq!(EMULATION_PATH, core::EMULATION_PATH);
        assert_eq!(MESSAGE_SIZE, core::MESSAGE_SIZE);
        assert_eq!(KIND_KEYBOARD, core::KIND_KEYBOARD);
        assert_eq!(KIND_CONSUMER, core::KIND_CONSUMER);
        assert_eq!(KIND_MOUSE, core::KIND_MOUSE);
        assert_eq!(KIND_SYSTEM, core::KIND_SYSTEM);
        assert_eq!(KEY_COUNT, core::KEY_COUNT);
        assert_eq!(ABI_VERSION, core::ABI_VERSION);
        assert_eq!(INDICATOR_NUM_LOCK, core::INDICATOR_NUM_LOCK);
        assert_eq!(INDICATOR_CAPS_LOCK, core::INDICATOR_CAPS_LOCK);
        assert_eq!(INDICATOR_SCROLL_LOCK, core::INDICATOR_SCROLL_LOCK);
        assert_eq!(KIND_HUMAN_INTERFACE, device::kind::HUMAN_INTERFACE);

        assert_eq!(GET_STATUS, core::GET_STATUS::IDENTIFIER.raw());
        assert_eq!(
            core::GET_STATUS::IDENTIFIER.get_kind(),
            device::kind::HUMAN_INTERFACE
        );
        assert_eq!(usize::from(core::GET_STATUS::IDENTIFIER.get_number()), 0);

        assert_eq!(size_of::<VersionAbi>(), size_of::<core::VersionAbi>());
        assert_eq!(size_of::<StatusAbi>(), size_of::<core::StatusAbi>());
        assert_eq!(
            encode_version_abi(&VersionAbi {
                version: ABI_VERSION,
                reserved: 0
            }),
            {
                let abi = core::VersionAbi {
                    version: core::ABI_VERSION,
                    reserved: 0,
                };
                let mut bytes = [0u8; 4];
                bytes[..2].copy_from_slice(&abi.version.to_le_bytes());
                bytes[2..].copy_from_slice(&abi.reserved.to_le_bytes());
                bytes
            }
        );

        // Modifier and button bits.
        assert_eq!(
            Modifiers::LEFT_CONTROL.bits(),
            core::Modifiers::LEFT_CONTROL.bits()
        );
        assert_eq!(
            Modifiers::LEFT_SHIFT.bits(),
            core::Modifiers::LEFT_SHIFT.bits()
        );
        assert_eq!(Modifiers::LEFT_ALT.bits(), core::Modifiers::LEFT_ALT.bits());
        assert_eq!(Modifiers::LEFT_GUI.bits(), core::Modifiers::LEFT_GUI.bits());
        assert_eq!(
            Modifiers::RIGHT_CONTROL.bits(),
            core::Modifiers::RIGHT_CONTROL.bits()
        );
        assert_eq!(
            Modifiers::RIGHT_SHIFT.bits(),
            core::Modifiers::RIGHT_SHIFT.bits()
        );
        assert_eq!(
            Modifiers::RIGHT_ALT.bits(),
            core::Modifiers::RIGHT_ALT.bits()
        );
        assert_eq!(
            Modifiers::RIGHT_GUI.bits(),
            core::Modifiers::RIGHT_GUI.bits()
        );
        assert_eq!(Modifiers::ALL.bits(), core::Modifiers::ALL.bits());
        assert_eq!(MouseButtons::LEFT.bits(), core::MouseButtons::LEFT.bits());
        assert_eq!(MouseButtons::RIGHT.bits(), core::MouseButtons::RIGHT.bits());
        assert_eq!(
            MouseButtons::MIDDLE.bits(),
            core::MouseButtons::MIDDLE.bits()
        );
        assert_eq!(MouseButtons::ALL.bits(), core::MouseButtons::ALL.bits());

        // Usage constants.
        assert_eq!(keyboard::A, core::keyboard::A);
        assert_eq!(keyboard::Z, core::keyboard::Z);
        assert_eq!(keyboard::NUMBER_1, core::keyboard::NUMBER_1);
        assert_eq!(keyboard::NUMBER_0, core::keyboard::NUMBER_0);
        assert_eq!(keyboard::ENTER, core::keyboard::ENTER);
        assert_eq!(keyboard::ESCAPE, core::keyboard::ESCAPE);
        assert_eq!(keyboard::BACKSPACE, core::keyboard::BACKSPACE);
        assert_eq!(keyboard::TAB, core::keyboard::TAB);
        assert_eq!(keyboard::SPACE, core::keyboard::SPACE);
        assert_eq!(keyboard::CAPS_LOCK, core::keyboard::CAPS_LOCK);
        assert_eq!(keyboard::F1, core::keyboard::F1);
        assert_eq!(keyboard::F12, core::keyboard::F12);
        assert_eq!(keyboard::PRINT_SCREEN, core::keyboard::PRINT_SCREEN);
        assert_eq!(keyboard::SCROLL_LOCK, core::keyboard::SCROLL_LOCK);
        assert_eq!(keyboard::PAUSE, core::keyboard::PAUSE);
        assert_eq!(keyboard::INSERT, core::keyboard::INSERT);
        assert_eq!(keyboard::HOME, core::keyboard::HOME);
        assert_eq!(keyboard::PAGE_UP, core::keyboard::PAGE_UP);
        assert_eq!(keyboard::DELETE, core::keyboard::DELETE);
        assert_eq!(keyboard::END, core::keyboard::END);
        assert_eq!(keyboard::PAGE_DOWN, core::keyboard::PAGE_DOWN);
        assert_eq!(keyboard::ARROW_RIGHT, core::keyboard::ARROW_RIGHT);
        assert_eq!(keyboard::ARROW_LEFT, core::keyboard::ARROW_LEFT);
        assert_eq!(keyboard::ARROW_DOWN, core::keyboard::ARROW_DOWN);
        assert_eq!(keyboard::ARROW_UP, core::keyboard::ARROW_UP);
        assert_eq!(keyboard::F13, core::keyboard::F13);
        assert_eq!(keyboard::F24, core::keyboard::F24);
        assert_eq!(consumer::PLAY_PAUSE, core::consumer::PLAY_PAUSE);
        assert_eq!(consumer::NEXT_TRACK, core::consumer::NEXT_TRACK);
        assert_eq!(consumer::PREVIOUS_TRACK, core::consumer::PREVIOUS_TRACK);
        assert_eq!(consumer::STOP, core::consumer::STOP);
        assert_eq!(consumer::VOLUME_UP, core::consumer::VOLUME_UP);
        assert_eq!(consumer::VOLUME_DOWN, core::consumer::VOLUME_DOWN);
        assert_eq!(consumer::MUTE, core::consumer::MUTE);
        assert_eq!(system::POWER_DOWN, core::system::POWER_DOWN);
        assert_eq!(system::SLEEP, core::system::SLEEP);
        assert_eq!(system::WAKE_UP, core::system::WAKE_UP);

        // The encoded messages are byte for byte the Core ones.
        let messages = [
            Message::Keyboard {
                modifiers: Modifiers::LEFT_GUI | Modifiers::LEFT_SHIFT,
                keys: [keyboard::S, 0, 0, 0, 0, 0],
            },
            Message::Keyboard {
                modifiers: Modifiers::NONE,
                keys: [0; KEY_COUNT],
            },
            Message::Consumer {
                usage: consumer::VOLUME_UP,
            },
            Message::Consumer { usage: 0 },
            Message::Mouse {
                buttons: MouseButtons::LEFT,
                x: -128,
                y: 127,
                wheel: 1,
                pan: -2,
            },
            Message::System {
                usage: system::POWER_DOWN,
            },
        ];
        let core_messages = [
            core::Message::Keyboard {
                modifiers: core::Modifiers::LEFT_GUI.union(core::Modifiers::LEFT_SHIFT),
                keys: [core::keyboard::S, 0, 0, 0, 0, 0],
            },
            core::Message::Keyboard {
                modifiers: core::Modifiers::empty(),
                keys: [0; core::KEY_COUNT],
            },
            core::Message::Consumer {
                usage: core::consumer::VOLUME_UP,
            },
            core::Message::Consumer { usage: 0 },
            core::Message::Mouse {
                buttons: core::MouseButtons::LEFT,
                x: -128,
                y: 127,
                wheel: 1,
                pan: -2,
            },
            core::Message::System {
                usage: core::system::POWER_DOWN,
            },
        ];
        for (message, core_message) in messages.iter().zip(core_messages.iter()) {
            assert_eq!(message.encode(), core_message.encode());
        }
    }
}
