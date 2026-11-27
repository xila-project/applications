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
}

impl CommandInput {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(State {
                attributes: TerminalAttributes::INTERACTIVE,
                queued: VecDeque::new(),
                updating_widget: false,
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
                    let key = Key::from(key_code);
                    let character;
                    let bytes: &[u8] = match key {
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
                        Key::Character(byte) if u32::from(byte) == key_code => {
                            character = [byte];
                            &character
                        }
                        _ => &[],
                    };
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
    Control(Option<char>),
}

/// Track the command text separately so the textarea can display its cursor.
/// ESC 7 starts command output, ESC 8 returns to its start, and CSI J clears
/// the old command. A newline commits the command text to the scrollback.
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
                (Escape::Start, '[') => self.escape = Escape::Control(None),
                (Escape::Control(None), '2') => self.escape = Escape::Control(Some('2')),
                (Escape::Control(Some('2')), 'J') => scrollback.clear(),
                (Escape::Control(None), 'J') if raw => {
                    if let Some((text, cursor)) = &mut self.edit {
                        text.truncate(*cursor);
                    }
                }
                (Escape::None, '\n') => {
                    if let Some((text, _)) = self.edit.take() {
                        scrollback.push_str(&text);
                    }
                    scrollback.push('\n');
                }
                (Escape::None, '\r' | '\x07' | '\0') => {}
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
}
