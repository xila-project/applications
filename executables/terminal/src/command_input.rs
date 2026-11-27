use alloc::collections::VecDeque;
use core::{cell::RefCell, ffi::CStr, mem};
use xila::{
    file_system::character_device::TerminalAttributes,
    graphics::{EventKind, Key, lvgl},
};

const MAXIMUM_QUEUED_BYTES: usize = 1024;

pub(crate) struct CommandInput {
    state: RefCell<State>,
}

struct State {
    attributes: TerminalAttributes,
    queued: VecDeque<u8>,
    updating_widget: bool,
    completion_focus_requested: bool,
}

/// Map an `LV_EVENT_KEY` key code to the bytes queued for the shell.
///
/// LVGL delivers full Unicode code points on `LV_EVENT_KEY`, so a character
/// above U+00FF arrives as its code point: queue its UTF-8 encoding instead of
/// truncating the code point to one byte.
fn key_to_bytes(key_code: u32, character: &mut [u8; 4]) -> &[u8] {
    match Key::from(key_code) {
        Key::Up => b"\x1b[A",
        Key::Down => b"\x1b[B",
        Key::Right => b"\x1b[C",
        Key::Left => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Delete => b"\x1b[3~",
        Key::Backspace => b"\x08",
        Key::Enter => b"\n",
        Key::Escape => b"\x1b",
        _ => match char::from_u32(key_code) {
            Some(value) => value.encode_utf8(character).as_bytes(),
            None => &[],
        },
    }
}

impl CommandInput {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(State {
                attributes: TerminalAttributes::INTERACTIVE,
                queued: VecDeque::new(),
                updating_widget: false,
                completion_focus_requested: false,
            }),
        }
    }

    pub fn attributes(&self) -> TerminalAttributes {
        self.state.borrow().attributes
    }

    pub fn set_attributes(&self, attributes: TerminalAttributes) {
        self.state.borrow_mut().attributes = attributes;
    }

    pub fn updating_widget(&self, update: impl FnOnce()) {
        self.state.borrow_mut().updating_widget = true;
        update();
        self.state.borrow_mut().updating_widget = false;
    }

    pub fn queue(&self, bytes: &[u8]) {
        let mut state = self.state.borrow_mut();
        if state.queued.len() + bytes.len() <= MAXIMUM_QUEUED_BYTES {
            state.queued.extend(bytes);
        }
    }

    pub fn drain_into(&self, destination: &mut VecDeque<u8>) {
        let mut state = self.state.borrow_mut();
        let count = state
            .queued
            .len()
            .min(MAXIMUM_QUEUED_BYTES.saturating_sub(destination.len()));
        destination.extend(state.queued.drain(..count));
    }

    pub fn take_completion_focus_request(&self) -> bool {
        mem::take(&mut self.state.borrow_mut().completion_focus_requested)
    }

    /// The textarea owns the callbacks. The event loop and display updates
    /// access their state only while holding the graphics lock.
    pub unsafe fn attach(&self, input: *mut lvgl::lv_obj_t) {
        use lvgl::*;
        let pointer = (self as *const Self).cast_mut().cast();
        unsafe {
            lv_obj_add_event_cb(
                input,
                Some(Self::on_input),
                lv_event_code_t_LV_EVENT_KEY | lv_event_code_t_LV_EVENT_PREPROCESS,
                pointer,
            );
            lv_obj_add_event_cb(
                input,
                Some(Self::on_input),
                lv_event_code_t_LV_EVENT_INSERT,
                pointer,
            );
            lv_obj_add_event_cb(
                input,
                Some(Self::on_input),
                lv_event_code_t_LV_EVENT_DEFOCUSED,
                pointer,
            );
        }
    }

    unsafe extern "C" fn on_input(event: *mut lvgl::lv_event_t) {
        use lvgl::*;
        unsafe {
            let input = &*lv_event_get_user_data(event).cast::<Self>();
            let state = input.state.borrow();
            if state.attributes.is_canonical() || state.updating_widget {
                return;
            }
            drop(state);
            match EventKind::from_lvgl_code(lv_event_get_code(event)) {
                EventKind::Key => {
                    let key_code = lv_event_get_key(event);
                    let mut character = [0; 4];
                    let bytes = key_to_bytes(key_code, &mut character);
                    input.queue(bytes);
                    lv_event_stop_processing(event);
                    lv_event_stop_bubbling(event);
                }
                EventKind::Insert => {
                    let text = CStr::from_ptr(lv_event_get_param(event).cast());
                    input.queue(text.to_bytes());
                    let target = lv_event_get_target(event).cast();
                    lv_textarea_set_insert_replace(target, c"".as_ptr());
                }
                EventKind::Defocused => {
                    // LVGL uses Tab to move focus before sending widget key events.
                    let device = lv_indev_active();
                    if !device.is_null()
                        && lv_indev_get_type(device) == lv_indev_type_t_LV_INDEV_TYPE_KEYPAD
                        && lv_indev_get_key(device) == lv_key_t_LV_KEY_NEXT
                    {
                        input.queue(b"\t");
                        input.state.borrow_mut().completion_focus_requested = true;
                    }
                }
                _ => {}
            }
        }
    }
}

#[derive(Default)]
enum Escape {
    #[default]
    None,
    Start,
    /// A `CSI` sequence: `number` accumulates the decimal parameter value,
    /// `other` marks a non-digit parameter byte, as the `?` of a private
    /// sequence.
    Control {
        number: u32,
        other: bool,
    },
}

/// Track the command text separately so the textarea can display its cursor.
/// ESC 7 starts command output, ESC 8 returns to its start, and CSI J clears
/// the old command. A newline commits the command text to the scrollback.
///
/// The shell redraws its line with relative cursor movement: `\r`, `CSI n A`,
/// `CSI n C` and `CSI J`. While the edit is tracked, `\r` moves the edit
/// cursor to the start of the line, and the other movements are consumed
/// without effect, so the text that follows overwrites the line from its
/// start. A newline commits the command text to the scrollback.
#[derive(Default)]
pub(crate) struct CommandOutput {
    edit: Option<(alloc::string::String, usize)>,
    escape: Escape,
}

impl CommandOutput {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn edit(&self) -> Option<(&str, usize)> {
        self.edit
            .as_ref()
            .map(|(text, cursor)| (text.as_str(), text[..*cursor].chars().count()))
    }

    pub fn write(&mut self, text: &str, scrollback: &mut alloc::string::String, raw: bool) {
        for character in text.chars() {
            match (mem::take(&mut self.escape), character) {
                (_, '\x1b') => self.escape = Escape::Start,
                (Escape::Start, '7') if raw => self.edit = Some((Default::default(), 0)),
                (Escape::Start, '8') if raw => {
                    if let Some((_, cursor)) = &mut self.edit {
                        *cursor = 0;
                    }
                }
                (Escape::Start, '[') => {
                    self.escape = Escape::Control {
                        number: 0,
                        other: false,
                    };
                }
                (Escape::Control { number, other }, byte) if byte.is_ascii_digit() => {
                    // Parameter bytes stay in the sequence; the final byte ends it.
                    self.escape = Escape::Control {
                        number: number
                            .saturating_mul(10)
                            .saturating_add(u32::from(byte) - u32::from('0')),
                        other,
                    };
                }
                (Escape::Control { number, .. }, byte) if ('\u{3a}'..='\u{3f}').contains(&byte) => {
                    self.escape = Escape::Control {
                        number,
                        other: true,
                    };
                }
                (
                    Escape::Control {
                        number: 0,
                        other: false,
                    },
                    'J',
                ) if raw => {
                    if let Some((text, cursor)) = &mut self.edit {
                        text.truncate(*cursor);
                    }
                }
                (
                    Escape::Control {
                        number: 2,
                        other: false,
                    },
                    'J',
                ) => scrollback.clear(),
                (Escape::None, '\n') => {
                    if let Some((text, _)) = self.edit.take() {
                        scrollback.push_str(&text);
                    }
                    scrollback.push('\n');
                }
                (Escape::None, '\r') => {
                    if let Some((_, cursor)) = &mut self.edit {
                        *cursor = 0;
                    }
                }
                (Escape::None, '\x07' | '\0') => {}
                (Escape::None, character) => {
                    if let Some((text, cursor)) = &mut self.edit {
                        let replaced_bytes =
                            text[*cursor..].chars().next().map_or(0, char::len_utf8);
                        text.replace_range(
                            *cursor..*cursor + replaced_bytes,
                            character.encode_utf8(&mut [0; 4]),
                        );
                        *cursor += character.len_utf8();
                    } else {
                        scrollback.push(character);
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_events_encode_full_unicode_code_points() {
        let mut character = [0; 4];
        // Named keys keep their escape sequences.
        assert_eq!(
            key_to_bytes(u32::from(Key::Left), &mut character),
            b"\x1b[D"
        );
        assert_eq!(key_to_bytes(u32::from(Key::Enter), &mut character), b"\n");
        // U+00E9 arrives as its code point and is queued as UTF-8, not as the
        // single byte 0xE9.
        assert_eq!(key_to_bytes(0xE9, &mut character), "é".as_bytes());
        // Code points above U+00FF are not truncated to one byte.
        assert_eq!(key_to_bytes(0x754C, &mut character), "界".as_bytes());
        assert_eq!(
            key_to_bytes(0x1F600, &mut character),
            "\u{1F600}".as_bytes()
        );
        // Surrogates are not characters and are dropped.
        assert_eq!(key_to_bytes(0xD800, &mut character), &[]);
    }

    #[test]
    fn command_redraw_keeps_the_prompt_and_positions_the_cursor() {
        let mut output = CommandOutput::default();
        let mut scrollback = alloc::string::String::new();
        output.write("prompt$ \x1b", &mut scrollback, true);
        output.write("7\x1b8\x1b[J界かな\x1b8界", &mut scrollback, true);
        assert_eq!(output.edit(), Some(("界かな", 1)));
        assert_eq!(scrollback, "prompt$ ");
        output.write("\x1b8\x1b[J界な\r\n", &mut scrollback, true);
        assert_eq!(output.edit(), None);
        assert_eq!(scrollback, "prompt$ 界な\n");
    }

    #[test]
    fn csi_parameters_are_parsed_as_a_full_number() {
        let mut output = CommandOutput::default();
        let mut scrollback = alloc::string::String::new();
        output.write("keep\x1b[2J", &mut scrollback, false);
        assert_eq!(scrollback, "");
        // CSI 20 J is not CSI 2 J: it must not clear the scrollback.
        output.write("keep\x1b[20Jmore", &mut scrollback, false);
        assert_eq!(scrollback, "keepmore");
    }

    #[test]
    fn relative_redraw_stream_replaces_the_line_and_places_the_cursor() {
        let mut output = CommandOutput::default();
        let mut scrollback = alloc::string::String::new();
        output.write("prompt$ \x1b7", &mut scrollback, true);
        // Carriage return, right to the start of the line, erase, the new
        // line, then back to the start and the text before the cursor.
        output.write("\r\x1b[8C\x1b[Jabc\r\x1b[8Ca", &mut scrollback, true);
        assert_eq!(output.edit(), Some(("abc", 1)));
        assert_eq!(scrollback, "prompt$ ");
        // Submitting redraws the line at the end of the input and commits it.
        output.write("\r\x1b[8C\x1b[Jabc\r\n", &mut scrollback, true);
        assert_eq!(output.edit(), None);
        assert_eq!(scrollback, "prompt$ abc\n");
    }
}
