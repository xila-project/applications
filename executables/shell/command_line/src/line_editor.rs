//! Bounded UTF-8 input and history for one interactive shell session.
use alloc::{collections::VecDeque, string::String};
use core::mem;

pub const MAXIMUM_LINE_BYTES: usize = 512;
pub const DEFAULT_HISTORY_CAPACITY: usize = 8;
pub const MAXIMUM_HISTORY_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Redraw,
    Submit,
    Cancel,
    EndOfInput,
    Bell,
}

#[derive(Default)]
enum Escape {
    #[default]
    None,
    Start,
    Parameters {
        number: Option<u8>,
        unsupported: bool,
    },
}

pub struct LineEditor {
    line: String,
    cursor: usize,
    history: VecDeque<String>,
    capacity: usize,
    recalled: Option<usize>,
    draft: String,
    escape: Escape,
    character: [u8; 4],
    character_length: usize,
    previous_was_carriage_return: bool,
    rejected: bool,
}

impl LineEditor {
    pub fn new(capacity: usize) -> Self {
        Self {
            line: String::new(),
            cursor: 0,
            history: VecDeque::new(),
            capacity: capacity.min(MAXIMUM_HISTORY_CAPACITY),
            recalled: None,
            draft: String::new(),
            escape: Escape::None,
            character: [0; 4],
            character_length: 0,
            previous_was_carriage_return: false,
            rejected: false,
        }
    }

    pub fn line(&self) -> &str {
        &self.line
    }

    pub fn before_cursor(&self) -> &str {
        &self.line[..self.cursor]
    }

    pub fn finish(&mut self) -> Option<String> {
        self.cursor = 0;
        self.recalled = None;
        self.draft.clear();
        self.escape = Escape::None;
        self.character_length = 0;
        let line = mem::take(&mut self.line);
        if mem::take(&mut self.rejected) {
            return None;
        }
        if self.capacity != 0 && !line.trim().is_empty() && self.history.back() != Some(&line) {
            if self.history.len() == self.capacity {
                self.history.pop_front();
            }
            self.history.push_back(line.clone());
        }
        Some(line)
    }

    pub fn discard(&mut self) {
        self.line.clear();
        self.rejected = false;
        self.finish();
    }

    fn recall(&mut self, previous: bool) -> Action {
        let line = match (self.recalled, previous) {
            (None, true) if !self.history.is_empty() => {
                self.draft.clone_from(&self.line);
                self.recalled = Some(self.history.len() - 1);
                &self.history[self.history.len() - 1]
            }
            (Some(index), true) if index > 0 => {
                self.recalled = Some(index - 1);
                &self.history[index - 1]
            }
            (Some(index), false) if index + 1 < self.history.len() => {
                self.recalled = Some(index + 1);
                &self.history[index + 1]
            }
            (Some(_), false) => {
                self.recalled = None;
                &self.draft
            }
            _ => return Action::None,
        };
        self.line.clone_from(line);
        self.cursor = self.line.len();
        Action::Redraw
    }

    fn previous_character(&self) -> usize {
        self.before_cursor()
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index)
    }

    fn next_character(&self) -> usize {
        self.cursor
            + self.line[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8)
    }

    fn move_cursor(&mut self, position: usize) -> Action {
        if self.cursor == position {
            return Action::None;
        }
        self.cursor = position;
        Action::Redraw
    }

    fn remove(&mut self, start: usize, end: usize) -> Action {
        if start == end {
            return Action::None;
        }
        self.line.replace_range(start..end, "");
        self.cursor = start;
        Action::Redraw
    }

    fn escape_key(&mut self, byte: u8, number: Option<u8>) -> Action {
        match (byte, number) {
            (b'A', None) => self.recall(true),
            (b'B', None) => self.recall(false),
            (b'C', None) => self.move_cursor(self.next_character()),
            (b'D', None) => self.move_cursor(self.previous_character()),
            (b'H', None) | (b'~', Some(1 | 7)) => self.move_cursor(0),
            (b'F', None) | (b'~', Some(4 | 8)) => self.move_cursor(self.line.len()),
            (b'~', Some(3)) => self.remove(self.cursor, self.next_character()),
            _ => Action::None,
        }
    }

    pub fn feed(&mut self, byte: u8) -> Action {
        if mem::take(&mut self.previous_was_carriage_return) && byte == b'\n' {
            return Action::None;
        }
        // These keys also recover from an incomplete escape or UTF-8 sequence.
        match byte {
            b'\r' | b'\n' => {
                self.previous_was_carriage_return = byte == b'\r';
                self.rejected |= self.character_length != 0;
                return Action::Submit;
            }
            3 => return Action::Cancel,
            4 if self.line.is_empty() && !self.rejected => return Action::EndOfInput,
            27 => {
                self.escape = Escape::Start;
                self.character_length = 0;
                return Action::None;
            }
            _ if self.rejected => return Action::None,
            _ => {}
        }

        match mem::take(&mut self.escape) {
            Escape::None => {}
            Escape::Start if matches!(byte, b'[' | b'O') => {
                self.escape = Escape::Parameters {
                    number: None,
                    unsupported: false,
                };
                return Action::None;
            }
            Escape::Start => return Action::None,
            Escape::Parameters {
                number,
                unsupported,
            } => {
                if (0x20..=0x3f).contains(&byte) {
                    self.escape = if byte.is_ascii_digit() && !unsupported {
                        Escape::Parameters {
                            number: Some(
                                number
                                    .unwrap_or(0)
                                    .saturating_mul(10)
                                    .saturating_add(byte - b'0'),
                            ),
                            unsupported: false,
                        }
                    } else {
                        Escape::Parameters {
                            number: None,
                            unsupported: true,
                        }
                    };
                    return Action::None;
                }
                return if unsupported {
                    Action::None
                } else {
                    self.escape_key(byte, number)
                };
            }
        }

        match byte {
            1 => self.move_cursor(0),
            5 => self.move_cursor(self.line.len()),
            8 | 127 => self.remove(self.previous_character(), self.cursor),
            11 => self.remove(self.cursor, self.line.len()),
            21 => self.remove(0, self.cursor),
            23 => {
                let start = self
                    .before_cursor()
                    .trim_end_matches(char::is_whitespace)
                    .rfind(char::is_whitespace)
                    .map_or(0, |index| self.next_boundary(index));
                self.remove(start, self.cursor)
            }
            32..=126 | 128..=255 => self.insert_byte(byte),
            _ => Action::None,
        }
    }

    fn next_boundary(&self, index: usize) -> usize {
        index + self.line[index..].chars().next().unwrap().len_utf8()
    }

    fn insert_byte(&mut self, byte: u8) -> Action {
        self.character[self.character_length] = byte;
        self.character_length += 1;
        match core::str::from_utf8(&self.character[..self.character_length]) {
            Ok(character) if self.line.len() + character.len() <= MAXIMUM_LINE_BYTES => {
                self.line.insert_str(self.cursor, character);
                self.cursor += character.len();
                self.character_length = 0;
                Action::Redraw
            }
            Err(error) if error.error_len().is_none() => Action::None,
            _ => {
                self.character_length = 0;
                self.rejected = true;
                Action::Bell
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_keys(editor: &mut LineEditor, keys: &str) {
        for byte in keys.bytes() {
            editor.feed(byte);
        }
    }

    #[test]
    fn edits_at_utf8_character_boundaries() {
        let mut editor = LineEditor::new(0);
        type_keys(&mut editor, "a界c\x1b[D\x1b[D★\x1b[3~");
        assert_eq!(editor.line(), "a★c");
        assert_eq!(editor.before_cursor(), "a★");
        type_keys(&mut editor, "\x1b[C\x08\x1b[H\x7fZ\x1bOF!");
        assert_eq!(editor.line(), "Za★!");
        assert_eq!(editor.before_cursor(), "Za★!");
    }

    #[test]
    fn control_keys_remove_text_on_either_side() {
        let mut editor = LineEditor::new(0);
        type_keys(&mut editor, "one two  three\x01\x1b[C\x15\x05\x17");
        assert_eq!(editor.line(), "ne two  ");
        type_keys(&mut editor, "\x17\x1b[D\x0b");
        assert_eq!(editor.line(), "ne");
        assert_eq!(editor.before_cursor(), "ne");
    }

    #[test]
    fn history_evicts_oldest_entries_and_restores_draft() {
        let mut editor = LineEditor::new(2);
        for command in ["one", "two", "two", " ", "three"] {
            type_keys(&mut editor, command);
            assert_eq!(editor.finish().as_deref(), Some(command));
        }
        type_keys(&mut editor, "draft\x1b[A\x1bOA\x1b[A");
        assert_eq!(editor.line(), "two");
        type_keys(&mut editor, "\x1b[B\x1b[B\x1b[B");
        assert_eq!(editor.line(), "draft");
        assert_eq!(editor.before_cursor(), "draft");
        assert_eq!(editor.history.len(), 2);
    }

    #[test]
    fn capacity_and_input_are_bounded() {
        assert_eq!(
            LineEditor::new(usize::MAX).capacity,
            MAXIMUM_HISTORY_CAPACITY
        );
        let mut editor = LineEditor::new(0);
        for _ in 0..MAXIMUM_LINE_BYTES {
            assert_eq!(editor.feed(b'x'), Action::Redraw);
        }
        assert_eq!(editor.feed(b'x'), Action::Bell);
        assert_eq!(editor.line().len(), MAXIMUM_LINE_BYTES);
        assert_eq!(editor.feed(b'\r'), Action::Submit);
        assert_eq!(editor.finish(), None);
        assert_eq!(editor.feed(b'\n'), Action::None);
        type_keys(&mut editor, "next");
        assert_eq!(editor.finish().as_deref(), Some("next"));
        assert!(editor.history.is_empty());
    }
}
