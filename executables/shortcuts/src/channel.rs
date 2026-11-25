//! The write protocol of the emulation device, independent of the transport.
//!
//! Each `write` carries one or more 8-byte messages. A write can return
//! `Busy` when the lower half is busy: the caller waits a short moment and
//! tries again. A short write means only the first messages were sent: the
//! caller sends the rest. A press is always followed by its release message,
//! also on errors where possible, so that no key stays pressed on the
//! computer.

use crate::error::{Error, Result};
use crate::hid::{
    self, ABI_VERSION, GET_STATUS, KEY_COUNT, Message, Modifiers, MouseButtons, StatusAbi,
    VersionAbi, decode_status_abi, encode_version_abi,
};
use alloc::vec::Vec;

/// How many times a busy write or control command is retried before the
/// error is returned to the user.
pub const MAX_BUSY_ATTEMPTS: usize = 64;

/// One opened emulation device, or anything that behaves like one in tests.
pub trait Channel {
    /// Attempts one write of `bytes` and returns the accepted byte count.
    fn write(&mut self, bytes: &[u8]) -> Result<usize>;

    /// Runs one control command: `input` is sent, `output` receives the
    /// answer.
    fn control(&mut self, command: u32, input: &[u8], output: &mut [u8]) -> Result<()>;

    /// Waits a short moment before a busy attempt is retried. The default
    /// does nothing, which is what a scripted test wants.
    fn pause(&mut self) {}
}

/// Writes every byte, retrying busy writes and continuing after short
/// writes.
pub fn write_all(channel: &mut impl Channel, mut bytes: &[u8]) -> Result<()> {
    while !bytes.is_empty() {
        let mut attempts = 0;
        loop {
            match channel.write(bytes) {
                // Nothing was accepted without an error: treat it as busy.
                Ok(0) | Err(Error::Busy) => {
                    attempts += 1;
                    if attempts >= MAX_BUSY_ATTEMPTS {
                        return Err(Error::Busy);
                    }
                    channel.pause();
                }
                Ok(count) => {
                    // A returned count larger than the slice or not a
                    // multiple of the message size is a broken device.
                    if count > bytes.len() || count % hid::MESSAGE_SIZE != 0 {
                        return Err(Error::Io);
                    }
                    bytes = &bytes[count..];
                    break;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

/// Encodes `messages` and sends them as one write, with retries and short
/// write handling.
pub fn send(channel: &mut impl Channel, messages: &[Message]) -> Result<()> {
    let mut bytes = Vec::with_capacity(messages.len() * hid::MESSAGE_SIZE);
    for message in messages {
        bytes.extend_from_slice(&message.encode());
    }
    write_all(channel, &bytes)
}

/// Connection state and keyboard indicators of the computer, as answered by
/// the `GET_STATUS` control command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    /// `true` when a computer is connected and listening.
    pub connected: bool,
    /// Keyboard indicator bitmask from the computer.
    pub indicators: u8,
}

impl Status {
    /// `true` when the caps lock indicator is on.
    pub fn caps_lock(&self) -> bool {
        self.indicators & hid::INDICATOR_CAPS_LOCK != 0
    }
}

/// Asks the device for the connection state, retrying busy answers.
pub fn query_status(channel: &mut impl Channel) -> Result<Status> {
    let input = encode_version_abi(&VersionAbi {
        version: ABI_VERSION,
        reserved: 0,
    });
    let mut output = [0u8; 8];
    let mut attempts = 0;
    loop {
        match channel.control(GET_STATUS, &input, &mut output) {
            Ok(()) => break,
            Err(Error::Busy) => {
                attempts += 1;
                if attempts >= MAX_BUSY_ATTEMPTS {
                    return Err(Error::Busy);
                }
                channel.pause();
            }
            Err(error) => return Err(error),
        }
    }
    let abi: StatusAbi = decode_status_abi(&output)?;
    Ok(Status {
        connected: abi.connected != 0,
        indicators: abi.indicators,
    })
}

/// Sends `press` and then `release`, also when the press fails, so that no
/// key or control stays pressed on the computer. The error of the press is
/// returned when it failed.
fn tap(channel: &mut impl Channel, press: Message, release: Message) -> Result<()> {
    let result = send(channel, &[press, release]);
    if result.is_err() {
        let _ = send(channel, &[release]);
    }
    result
}

/// Presses and releases a keyboard shortcut.
pub fn tap_keys(
    channel: &mut impl Channel,
    modifiers: Modifiers,
    keys: [u8; KEY_COUNT],
) -> Result<()> {
    tap(
        channel,
        Message::Keyboard { modifiers, keys },
        Message::Keyboard {
            modifiers: Modifiers::NONE,
            keys: [0; KEY_COUNT],
        },
    )
}

/// Presses and releases a consumer control usage, such as play/pause.
pub fn tap_consumer(channel: &mut impl Channel, usage: u16) -> Result<()> {
    tap(
        channel,
        Message::Consumer { usage },
        Message::Consumer { usage: 0 },
    )
}

/// Presses and releases a system control usage, such as power down.
pub fn tap_system(channel: &mut impl Channel, usage: u8) -> Result<()> {
    tap(
        channel,
        Message::System { usage },
        Message::System { usage: 0 },
    )
}

/// Sends one relative mouse movement with the given buttons held.
pub fn mouse_move(channel: &mut impl Channel, buttons: MouseButtons, x: i8, y: i8) -> Result<()> {
    send(
        channel,
        &[Message::Mouse {
            buttons,
            x,
            y,
            wheel: 0,
            pan: 0,
        }],
    )
}

/// Presses and releases mouse buttons, a click.
pub fn mouse_click(channel: &mut impl Channel, buttons: MouseButtons) -> Result<()> {
    tap(
        channel,
        Message::Mouse {
            buttons,
            x: 0,
            y: 0,
            wheel: 0,
            pan: 0,
        },
        Message::Mouse {
            buttons: MouseButtons::NONE,
            x: 0,
            y: 0,
            wheel: 0,
            pan: 0,
        },
    )
}

/// Sends one vertical wheel step, positive up.
pub fn mouse_scroll(channel: &mut impl Channel, wheel: i8) -> Result<()> {
    send(
        channel,
        &[Message::Mouse {
            buttons: MouseButtons::NONE,
            x: 0,
            y: 0,
            wheel,
            pan: 0,
        }],
    )
}

/// Sends the all-zero release messages of keyboard, consumer control and
/// mouse, as a safety net at start and at exit: no key, control or button
/// stays pressed on the computer.
pub fn release_all(channel: &mut impl Channel) -> Result<()> {
    send(
        channel,
        &[
            Message::Keyboard {
                modifiers: Modifiers::NONE,
                keys: [0; KEY_COUNT],
            },
            Message::Consumer { usage: 0 },
            Message::Mouse {
                buttons: MouseButtons::NONE,
                x: 0,
                y: 0,
                wheel: 0,
                pan: 0,
            },
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::{consumer, keyboard, system};
    use alloc::vec;
    use alloc::vec::Vec;

    /// Scripted channel: records every accepted write, can answer busy a
    /// chosen number of times, can accept only part of a write, and answers
    /// the status command with a fixed payload.
    struct Scripted {
        writes: Vec<Vec<u8>>,
        busy_writes: usize,
        busy_controls: usize,
        maximum_write: usize,
        pauses: usize,
        status: [u8; 8],
    }

    impl Scripted {
        fn connected() -> Self {
            Self {
                writes: Vec::new(),
                busy_writes: 0,
                busy_controls: 0,
                maximum_write: usize::MAX,
                pauses: 0,
                status: {
                    let mut bytes = [0u8; 8];
                    bytes[..2].copy_from_slice(&ABI_VERSION.to_le_bytes());
                    bytes[2] = 1;
                    bytes[3] = hid::INDICATOR_CAPS_LOCK;
                    bytes
                },
            }
        }

        fn bytes(&self) -> Vec<u8> {
            self.writes.concat()
        }
    }

    impl Channel for Scripted {
        fn write(&mut self, bytes: &[u8]) -> Result<usize> {
            if self.busy_writes > 0 {
                self.busy_writes -= 1;
                return Err(Error::Busy);
            }
            let count = bytes.len().min(self.maximum_write);
            self.writes.push(bytes[..count].to_vec());
            Ok(count)
        }

        fn control(&mut self, _command: u32, input: &[u8], output: &mut [u8]) -> Result<()> {
            assert_eq!(
                input,
                encode_version_abi(&VersionAbi {
                    version: ABI_VERSION,
                    reserved: 0
                })
            );
            if self.busy_controls > 0 {
                self.busy_controls -= 1;
                return Err(Error::Busy);
            }
            output.copy_from_slice(&self.status);
            Ok(())
        }

        fn pause(&mut self) {
            self.pauses += 1;
        }
    }

    #[test]
    fn tap_keys_sends_the_press_and_the_release() {
        let mut channel = Scripted::connected();
        tap_keys(
            &mut channel,
            Modifiers::LEFT_CONTROL,
            [keyboard::C, 0, 0, 0, 0, 0],
        )
        .unwrap();
        assert_eq!(
            channel.bytes(),
            vec![
                1,
                0x01,
                keyboard::C,
                0,
                0,
                0,
                0,
                0, // press
                1,
                0,
                0,
                0,
                0,
                0,
                0,
                0, // release
            ]
        );
    }

    #[test]
    fn tap_consumer_and_system_send_usage_and_release() {
        let mut channel = Scripted::connected();
        tap_consumer(&mut channel, consumer::PLAY_PAUSE).unwrap();
        tap_system(&mut channel, system::POWER_DOWN).unwrap();
        assert_eq!(
            channel.bytes(),
            vec![
                2, 0xCD, 0x00, 0, 0, 0, 0, 0, // play/pause press
                2, 0, 0, 0, 0, 0, 0, 0, // release
                4, 0x81, 0, 0, 0, 0, 0, 0, // power down press
                4, 0, 0, 0, 0, 0, 0, 0, // release
            ]
        );
    }

    #[test]
    fn busy_writes_are_retried_after_a_pause() {
        let mut channel = Scripted::connected();
        channel.busy_writes = 3;
        tap_consumer(&mut channel, consumer::NEXT_TRACK).unwrap();
        assert_eq!(channel.pauses, 3);
        assert_eq!(
            channel.bytes(),
            vec![2, 0xB5, 0x00, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]
        );
    }

    #[test]
    fn busy_writes_give_up_after_the_maximum_attempts() {
        let mut channel = Scripted::connected();
        channel.busy_writes = MAX_BUSY_ATTEMPTS;
        assert_eq!(tap_consumer(&mut channel, consumer::MUTE), Err(Error::Busy));
        assert_eq!(channel.pauses, MAX_BUSY_ATTEMPTS - 1);
        // The release is still attempted after the failed press, and this
        // time the device accepts it.
        assert_eq!(channel.writes.len(), 1);
        assert_eq!(channel.writes[0], vec![2, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn short_writes_send_the_rest() {
        let mut channel = Scripted::connected();
        channel.maximum_write = 8; // only the first message of a write fits
        tap_keys(
            &mut channel,
            Modifiers::LEFT_GUI | Modifiers::LEFT_SHIFT,
            [keyboard::S, 0, 0, 0, 0, 0],
        )
        .unwrap();
        assert_eq!(channel.writes.len(), 2);
        assert_eq!(
            channel.bytes(),
            vec![
                1,
                0x0A,
                keyboard::S,
                0,
                0,
                0,
                0,
                0, // press
                1,
                0,
                0,
                0,
                0,
                0,
                0,
                0, // release
            ]
        );
    }

    #[test]
    fn a_failing_press_still_sends_the_release() {
        /// Refuses every write whose first message is a press, accepts the
        /// release messages, and records every attempted write.
        struct FailingOnPress {
            writes: Vec<Vec<u8>>,
        }
        impl Channel for FailingOnPress {
            fn write(&mut self, bytes: &[u8]) -> Result<usize> {
                self.writes.push(bytes.to_vec());
                let first = &bytes[..hid::MESSAGE_SIZE];
                let is_release = first[1..].iter().all(|&byte| byte == 0);
                if is_release {
                    Ok(bytes.len())
                } else {
                    Err(Error::NotConnected)
                }
            }
            fn control(&mut self, _command: u32, _input: &[u8], _output: &mut [u8]) -> Result<()> {
                Ok(())
            }
        }
        let mut channel = FailingOnPress { writes: Vec::new() };
        assert_eq!(
            tap_keys(&mut channel, Modifiers::NONE, [keyboard::A, 0, 0, 0, 0, 0]),
            Err(Error::NotConnected)
        );
        // The press write failed, then the release was sent alone.
        assert_eq!(
            channel.writes,
            vec![
                vec![1, 0, keyboard::A, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0],
                vec![1, 0, 0, 0, 0, 0, 0, 0],
            ]
        );
    }

    #[test]
    fn query_status_decodes_the_answer_and_retries_busy() {
        let mut channel = Scripted::connected();
        channel.busy_controls = 2;
        let status = query_status(&mut channel).unwrap();
        assert_eq!(
            status,
            Status {
                connected: true,
                indicators: hid::INDICATOR_CAPS_LOCK,
            }
        );
        assert!(status.caps_lock());
        assert_eq!(channel.pauses, 2);

        let mut channel = Scripted::connected();
        channel.status[2] = 0;
        channel.status[3] = 0;
        let status = query_status(&mut channel).unwrap();
        assert_eq!(
            status,
            Status {
                connected: false,
                indicators: 0
            }
        );
        assert!(!status.caps_lock());

        let mut channel = Scripted::connected();
        channel.status[0] = 9; // wrong version
        assert_eq!(query_status(&mut channel), Err(Error::InvalidStatus));
    }

    #[test]
    fn a_broken_count_is_an_error() {
        /// Accepts a count larger than the slice, or not a multiple of 8.
        struct BrokenCount {
            count: usize,
        }
        impl Channel for BrokenCount {
            fn write(&mut self, bytes: &[u8]) -> Result<usize> {
                let _ = bytes;
                Ok(self.count)
            }
            fn control(&mut self, _command: u32, _input: &[u8], _output: &mut [u8]) -> Result<()> {
                Ok(())
            }
        }
        let mut channel = BrokenCount { count: 100 };
        assert_eq!(tap_consumer(&mut channel, consumer::MUTE), Err(Error::Io));
        let mut channel = BrokenCount { count: 12 };
        assert_eq!(tap_consumer(&mut channel, consumer::MUTE), Err(Error::Io));
    }

    #[test]
    fn release_all_sends_the_three_zero_messages() {
        let mut channel = Scripted::connected();
        release_all(&mut channel).unwrap();
        assert_eq!(
            channel.bytes(),
            vec![
                1, 0, 0, 0, 0, 0, 0, 0, // keyboard release
                2, 0, 0, 0, 0, 0, 0, 0, // consumer release
                3, 0, 0, 0, 0, 0, 0, 0, // mouse release
            ]
        );
    }

    #[test]
    fn mouse_helpers_encode_movement_click_and_scroll() {
        let mut channel = Scripted::connected();
        mouse_move(&mut channel, MouseButtons::NONE, -5, 120).unwrap();
        mouse_click(&mut channel, MouseButtons::LEFT).unwrap();
        mouse_scroll(&mut channel, -1).unwrap();
        assert_eq!(
            channel.bytes(),
            vec![
                3, 0, 0xFB, 0x78, 0, 0, 0, 0, // move
                3, 1, 0, 0, 0, 0, 0, 0, // left button press
                3, 0, 0, 0, 0, 0, 0, 0, // release
                3, 0, 0, 0, 0xFF, 0, 0, 0, // one wheel step down
            ]
        );
    }
}
