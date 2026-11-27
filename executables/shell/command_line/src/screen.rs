//! Relative cursor movement for redrawing the interactive command line.
//!
//! The redraw never saves and restores an absolute screen position: when a
//! long line wraps past the bottom row the screen scrolls, and a saved
//! position then points at the wrong row. The shell instead tracks the cell
//! of the cursor from the prompt width, the line width and the terminal
//! width, and moves relative to the current cursor: carriage return plus
//! `CSI n A` up to the row of the line, `CSI J` to erase the old text
//! including wrapped rows, then the new line and the cursor placement.

use alloc::{format, string::String};

/// Terminal width in columns used to compute wrapped rows.
///
/// No device control reports the terminal size yet, so the shell assumes 80
/// columns, the usual default.
pub(crate) const ASSUMED_TERMINAL_COLUMNS: usize = 80;

/// Display width of `text`, one column per character.
pub(crate) fn width(text: &str) -> usize {
    text.chars().count()
}

/// Cell of the cursor after `offset` columns were written from column 0 of
/// the prompt row. Terminals wrap lazily: the cursor stays in the last column
/// after filling a row and wraps when the next character is written.
pub(crate) fn cell(offset: usize, columns: usize) -> (usize, usize) {
    if offset == 0 {
        (0, 0)
    } else if offset.is_multiple_of(columns) {
        (offset / columns - 1, columns - 1)
    } else {
        (offset / columns, offset % columns)
    }
}

/// Append a relative move of the cursor from one cell to another.
fn move_to(out: &mut String, from: (usize, usize), to: (usize, usize)) {
    if from == to {
        return;
    }
    // `\r` first, so the vertical moves keep column 0 and land on `to`.
    out.push('\r');
    if from.0 > to.0 {
        out.push_str(&format!("\x1b[{}A", from.0 - to.0));
    } else if to.0 > from.0 {
        out.push_str(&format!("\x1b[{}B", to.0 - from.0));
    }
    if to.1 > 0 {
        out.push_str(&format!("\x1b[{}C", to.1));
    }
}

/// Append the byte stream that erases the edited line, keeping the prompt.
///
/// `from_columns` is the display width of the text before the cursor in the
/// line currently shown on the screen.
pub(crate) fn erase_line(out: &mut String, prompt: &str, from_columns: usize, columns: usize) {
    let prompt_columns = width(prompt);
    move_to(
        out,
        cell(prompt_columns + from_columns, columns),
        cell(prompt_columns, columns),
    );
    out.push_str("\x1b[J");
}

/// Append the byte stream that redraws the edited line and places the cursor.
///
/// `from_columns` is the display width of the text before the cursor in the
/// line currently shown on the screen; `cursor` is the byte length of the
/// line before the cursor after the redraw. The stream moves to the start of
/// the line, erases the old text including wrapped rows, writes the line,
/// and, when the cursor is inside the line, returns to the start and replays
/// the text before the cursor to place it.
pub(crate) fn redraw(
    out: &mut String,
    prompt: &str,
    from_columns: usize,
    line: &str,
    cursor: usize,
    columns: usize,
) {
    let prompt_columns = width(prompt);
    let start = cell(prompt_columns, columns);
    let from = cell(prompt_columns + from_columns, columns);
    let end = cell(prompt_columns + width(line), columns);
    move_to(out, from, start);
    out.push_str("\x1b[J");
    out.push_str(line);
    if width(&line[..cursor]) < width(line) {
        move_to(out, end, start);
        out.push_str(&line[..cursor]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec, vec::Vec};

    /// A small terminal screen: a grid that scrolls, with lazy wrapping.
    struct Model {
        columns: usize,
        rows: Vec<Vec<char>>,
        row: usize,
        column: usize,
        wrap_pending: bool,
    }

    impl Model {
        fn new(columns: usize, height: usize) -> Self {
            Self {
                columns,
                rows: vec![vec![' '; columns]; height],
                row: 0,
                column: 0,
                wrap_pending: false,
            }
        }

        fn scroll(&mut self) {
            self.rows.remove(0);
            self.rows.push(vec![' '; self.columns]);
            self.row -= 1;
        }

        fn put(&mut self, character: char) {
            if self.wrap_pending {
                self.wrap_pending = false;
                self.column = 0;
                self.row += 1;
                if self.row == self.rows.len() {
                    self.scroll();
                }
            }
            self.rows[self.row][self.column] = character;
            if self.column == self.columns - 1 {
                self.wrap_pending = true;
            } else {
                self.column += 1;
            }
        }

        fn feed(&mut self, text: &str) {
            let mut characters = text.chars();
            while let Some(character) = characters.next() {
                match character {
                    '\x1b' => {
                        let mut sequence = String::new();
                        for next in characters.by_ref() {
                            // The CSI introducer is not a parameter byte.
                            if next != '[' {
                                sequence.push(next);
                            }
                            if next.is_ascii_alphabetic() {
                                break;
                            }
                        }
                        self.control(&sequence);
                    }
                    '\r' => {
                        self.column = 0;
                        self.wrap_pending = false;
                    }
                    '\n' => {
                        self.row += 1;
                        if self.row == self.rows.len() {
                            self.scroll();
                        }
                        self.wrap_pending = false;
                    }
                    '\x07' => {}
                    _ => self.put(character),
                }
            }
        }

        fn control(&mut self, sequence: &str) {
            let (parameters, final_byte) = sequence.split_at(sequence.len() - 1);
            let number: usize = parameters.parse().unwrap_or(1);
            self.wrap_pending = false;
            match final_byte {
                // Up, down and right keep the column; the carriage return was
                // handled before the sequence.
                "A" => self.row = self.row.saturating_sub(number),
                "B" => self.row = (self.row + number).min(self.rows.len() - 1),
                "C" => self.column = (self.column + number).min(self.columns - 1),
                // Erase from the cursor to the end of the screen.
                "J" => {
                    for row in self.row..self.rows.len() {
                        let start = if row == self.row { self.column } else { 0 };
                        for column in start..self.columns {
                            self.rows[row][column] = ' ';
                        }
                    }
                }
                _ => {}
            }
        }

        fn text(&self, row: usize) -> String {
            self.rows[row].iter().collect()
        }

        fn prompt_row(&self, prompt: &str) -> usize {
            (0..self.rows.len())
                .find(|row| self.text(*row).starts_with(prompt))
                .expect("the prompt is on the screen")
        }
    }

    #[test]
    fn redraw_moves_relative_to_the_cursor() {
        let mut out = String::new();
        redraw(&mut out, "ab$ ", 2, "hello", 2, 80);
        assert_eq!(out, "\r\x1b[4C\x1b[Jhello\r\x1b[4Che");

        let mut out = String::new();
        redraw(&mut out, "ab$ ", 100, &"x".repeat(100), 100, 80);
        assert_eq!(out, "\r\x1b[1A\x1b[4C\x1b[J".to_string() + &"x".repeat(100));

        let mut out = String::new();
        erase_line(&mut out, "ab$ ", 2, 80);
        assert_eq!(out, "\r\x1b[4C\x1b[J");
    }

    /// Type a 100 character line on a prompt at the bottom row, then move
    /// the cursor inside it, and check the wrapped screen and the cursor.
    fn check_wrapped_redraw(columns: usize) {
        let height = 6;
        let mut model = Model::new(columns, height);
        for _ in 1..height {
            model.feed("output line\r\n");
        }
        let prompt = "root@xila:/$ ";
        model.feed(prompt);
        model.feed("\x1b7");
        let line = "x".repeat(100);
        let mut from_columns = 0;
        for length in 0..=line.len() {
            let mut stream = String::new();
            redraw(
                &mut stream,
                prompt,
                from_columns,
                &line[..length],
                length,
                columns,
            );
            model.feed(&stream);
            from_columns = length;
        }
        check_screen_and_cursor(&model, prompt, &line, line.len(), columns);
        for cursor in [50, 0] {
            let mut stream = String::new();
            redraw(&mut stream, prompt, from_columns, &line, cursor, columns);
            model.feed(&stream);
            from_columns = cursor;
            check_screen_and_cursor(&model, prompt, &line, cursor, columns);
        }
    }

    fn check_screen_and_cursor(
        model: &Model,
        prompt: &str,
        line: &str,
        cursor: usize,
        columns: usize,
    ) {
        let prompt_row = model.prompt_row(prompt);
        // The screen shows the prompt and the line wrapped at `columns`.
        let mut expected = String::from(prompt);
        expected.push_str(line);
        for (index, chunk) in expected.as_bytes().chunks(columns).enumerate() {
            let chunk = core::str::from_utf8(chunk).unwrap();
            let mut row = String::from(chunk);
            row.push_str(&" ".repeat(columns - chunk.len()));
            assert_eq!(model.text(prompt_row + index), row, "row {index}");
        }
        // The cursor sits where the tracked cell says, relative to the
        // scrolled prompt row, not at a stale saved position.
        let expected_cell = cell(width(prompt) + cursor, columns);
        assert_eq!(
            (model.row, model.column),
            (prompt_row + expected_cell.0, expected_cell.1)
        );
    }

    #[test]
    fn wrapped_redraw_from_the_bottom_row_at_80_columns() {
        check_wrapped_redraw(80);
    }

    #[test]
    fn wrapped_redraw_from_the_bottom_row_at_40_columns() {
        check_wrapped_redraw(40);
    }

    #[test]
    fn cells_follow_lazy_wrapping() {
        assert_eq!(cell(0, 80), (0, 0));
        assert_eq!(cell(79, 80), (0, 79));
        assert_eq!(cell(80, 80), (0, 79));
        assert_eq!(cell(81, 80), (1, 1));
        assert_eq!(cell(160, 80), (1, 79));
        assert_eq!(cell(161, 80), (2, 1));
    }
}
