//! Relative cursor movement for redrawing the interactive command line.
//!
//! The redraw never saves and restores an absolute screen position: when a
//! long line wraps past the bottom row the screen scrolls, and a saved
//! position then points at the wrong row. The shell instead tracks the cell
//! of the cursor from the prompt and the line, laid out at the terminal
//! width, and moves relative to the current cursor: carriage return plus
//! `CSI n A` up to the row of the line, `CSI J` to erase the old text
//! including wrapped rows, then the new line and the cursor placement.

use alloc::{format, string::String};

/// Terminal width in columns used to compute wrapped rows.
///
/// No device control reports the terminal size yet, so the shell assumes 80
/// columns, the usual default.
pub(crate) const ASSUMED_TERMINAL_COLUMNS: usize = 80;

/// Display width of one character. East Asian Wide and Fullwidth characters
/// take two columns, combining marks and zero-width characters take none,
/// other printable characters take one, and control characters take none.
///
/// The widths come from a small table of the common ranges, not from the
/// `unicode-width` crate: its tables would add around 15 KiB of flash to
/// every executable that links the shell, and the tightest target
/// (xila-project/platforms#200) has no room for them. The table covers CJK
/// ideographs and radicals, Hiragana, Katakana, Bopomofo, Hangul, the Yi
/// script, CJK compatibility ideographs and forms, the fullwidth forms and
/// signs, emoji, and the combining diacritical marks; it does not cover the
/// less common wide ranges (Tangut, Egyptian hieroglyphs, further CJK
/// extension blocks) or the other combining blocks (for example Devanagari
/// or Hebrew marks), which count as one column.
fn char_width(character: char) -> usize {
    if character.is_control() {
        return 0;
    }
    let value = character as u32;
    if (0x1100..=0x115F).contains(&value)
        || (0x2E80..=0x303E).contains(&value)
        || (0x3041..=0x33FF).contains(&value)
        || (0x3400..=0x4DBF).contains(&value)
        || (0x4E00..=0x9FFF).contains(&value)
        || (0xA000..=0xA4CF).contains(&value)
        || (0xA960..=0xA97F).contains(&value)
        || (0xAC00..=0xD7A3).contains(&value)
        || (0xF900..=0xFAFF).contains(&value)
        || (0xFE30..=0xFE4F).contains(&value)
        || (0xFF00..=0xFF60).contains(&value)
        || (0xFFE0..=0xFFE6).contains(&value)
        || (0x1F300..=0x1F9FF).contains(&value)
        || (0x20000..=0x3FFFD).contains(&value)
    {
        2
    } else if (0x0300..=0x036F).contains(&value)
        || (0x200B..=0x200F).contains(&value)
        || (0x20D0..=0x20FF).contains(&value)
        || (0xFE00..=0xFE0F).contains(&value)
    {
        0
    } else {
        1
    }
}

/// Cell offset from column 0 of the prompt row after laying out `prompt` and
/// then `text` at `columns` cells per row. Escape sequences and control
/// characters take no cells. A double width character that does not fit in
/// the remaining cells of a row wraps to the next row, leaving the last
/// cell of the row empty.
pub(crate) fn position(prompt: &str, text: &str, columns: usize) -> usize {
    let mut position = 0;
    let mut characters = prompt.chars().chain(text.chars());
    while let Some(character) = characters.next() {
        if character == '\x1b' {
            // Skip an escape sequence: a CSI sequence ends at its final byte
            // in 0x40..=0x7E, any other escape at the next character.
            if characters.next() == Some('[') {
                for next in characters.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        let width = char_width(character);
        if width == 0 {
            continue;
        }
        if width == 2 && position % columns == columns - 1 {
            position += 1;
        }
        position += width;
    }
    position
}

/// Logical cell of a cell offset: the cell where the next character goes.
pub(crate) fn cell(offset: usize, columns: usize) -> (usize, usize) {
    (offset / columns, offset % columns)
}

/// Cell the terminal cursor occupies after printing up to `offset` cells.
/// Terminals wrap lazily: a row filled exactly leaves the cursor pending in
/// its last column, and the next printed character wraps to the next row.
pub(crate) fn physical_cell(offset: usize, columns: usize) -> (usize, usize) {
    if offset > 0 && offset.is_multiple_of(columns) {
        (offset / columns - 1, columns - 1)
    } else {
        cell(offset, columns)
    }
}

/// Append a relative move of the cursor between logical cells. The carriage
/// return also clears a pending wrap, so printing after the move never wraps
/// unexpectedly.
fn move_to(out: &mut String, from: (usize, usize), to: (usize, usize)) {
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

/// Append the byte stream that takes the cursor from a pending wrap to the
/// logical cell of `offset`: after printing exactly to the end of a row, the
/// terminal leaves the cursor pending in the last column. The carriage
/// return clears the pending wrap and the line feed moves to the next row,
/// scrolling the screen when the cursor was on the bottom row, which a
/// cursor-down sequence never does. The move keeps the tracked cell exact.
pub(crate) fn leave_pending_wrap(out: &mut String, offset: usize, columns: usize) {
    if offset > 0 && offset.is_multiple_of(columns) {
        out.push_str("\r\n");
    }
}

/// Append the byte stream that erases the edited line, keeping the prompt.
///
/// `from_position` is the cell offset of the cursor in the line currently
/// shown on the screen.
pub(crate) fn erase_line(out: &mut String, prompt: &str, from_position: usize, columns: usize) {
    let start = position(prompt, "", columns);
    move_to(out, cell(from_position, columns), cell(start, columns));
    out.push_str("\x1b[J");
}

/// Append the byte stream that redraws the edited line and places the cursor.
///
/// `from_position` is the cell offset of the cursor in the line currently
/// shown on the screen; `cursor` is the byte length of the line before the
/// cursor after the redraw. The stream moves to the start of the line,
/// erases the old text including wrapped rows, writes the line, and, when
/// the cursor is inside the line, returns to the start and replays the text
/// before the cursor to place it.
pub(crate) fn redraw(
    out: &mut String,
    prompt: &str,
    from_position: usize,
    line: &str,
    cursor: usize,
    columns: usize,
) {
    let start = position(prompt, "", columns);
    let end = position(prompt, line, columns);
    let cursor_position = position(prompt, &line[..cursor], columns);
    move_to(out, cell(from_position, columns), cell(start, columns));
    out.push_str("\x1b[J");
    out.push_str(line);
    if cursor_position < end {
        move_to(out, physical_cell(end, columns), cell(start, columns));
        out.push_str(&line[..cursor]);
    }
    // Printing exactly to the end of a row leaves the cursor pending in the
    // last column; move it to the logical cell of the cursor.
    if cursor_position > start && cursor_position.is_multiple_of(columns) {
        leave_pending_wrap(out, cursor_position, columns);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec, vec::Vec};

    /// Marks the second cell of a double width character.
    const CONTINUATION: char = '\u{0}';

    /// A small terminal screen: a grid that scrolls, with lazy wrapping and
    /// double width characters.
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
            let width = char_width(character);
            if width == 0 {
                // A combining mark overlays the previous cell.
                return;
            }
            if self.wrap_pending || (width == 2 && self.column + 1 >= self.columns) {
                self.wrap_pending = false;
                self.column = 0;
                self.row += 1;
                if self.row == self.rows.len() {
                    self.scroll();
                }
            }
            self.rows[self.row][self.column] = character;
            if width == 2 {
                self.rows[self.row][self.column + 1] = CONTINUATION;
                self.column += 2;
            } else {
                self.column += 1;
            }
            if self.column >= self.columns {
                self.column = self.columns - 1;
                self.wrap_pending = true;
            }
        }

        fn feed(&mut self, text: &str) {
            let mut characters = text.chars();
            while let Some(character) = characters.next() {
                match character {
                    '\x1b' => {
                        // A CSI sequence ends at its final byte in
                        // 0x40..=0x7E; any other escape, as ESC 7, is two
                        // characters.
                        if characters.next() == Some('[') {
                            let mut sequence = String::new();
                            for next in characters.by_ref() {
                                sequence.push(next);
                                if ('\u{40}'..='\u{7e}').contains(&next) {
                                    break;
                                }
                            }
                            self.control(&sequence);
                        }
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
            let head: String = prompt.chars().take(self.columns).collect();
            (0..self.rows.len())
                .find(|row| self.text(*row).starts_with(&head))
                .expect("the prompt is on the screen")
        }
    }

    /// Render `text` into screen rows of `columns` cells, as the terminal
    /// lays it out: double width characters take two cells, combining marks
    /// none, and a double width character that does not fit wraps.
    fn render(text: &str, columns: usize) -> Vec<String> {
        let mut rows: Vec<String> = vec![String::new()];
        for character in text.chars() {
            let width = char_width(character);
            if width == 0 {
                continue;
            }
            loop {
                let row = rows.last_mut().unwrap();
                let used = row.chars().count();
                if used + width <= columns && !(width == 2 && used == columns - 1) {
                    row.push(character);
                    if width == 2 {
                        row.push(CONTINUATION);
                    }
                    break;
                }
                rows.push(String::new());
            }
        }
        rows.iter()
            .map(|row| {
                let mut padded = row.clone();
                while padded.chars().count() < columns {
                    padded.push(' ');
                }
                padded
            })
            .collect()
    }

    /// Feed the prompt as the shell writes it, including the move out of a
    /// pending wrap when the prompt fills a row exactly.
    fn feed_prompt(model: &mut Model, prompt: &str, columns: usize) {
        model.feed(prompt);
        model.feed("\x1b7");
        let mut sequence = String::new();
        leave_pending_wrap(&mut sequence, position(prompt, "", columns), columns);
        model.feed(&sequence);
    }

    /// Check that the screen shows the prompt and the line laid out at
    /// `columns`, and that the cursor sits at its tracked cell.
    fn check_screen_and_cursor(
        model: &Model,
        prompt: &str,
        line: &str,
        cursor: usize,
        columns: usize,
    ) {
        let prompt_row = model.prompt_row(prompt);
        let mut expected = String::from(prompt);
        expected.push_str(line);
        for (index, row) in render(&expected, columns).iter().enumerate() {
            assert_eq!(&model.text(prompt_row + index), row, "row {index}");
        }
        let expected_cell = cell(position(prompt, &line[..cursor], columns), columns);
        assert_eq!(
            (model.row, model.column),
            (prompt_row + expected_cell.0, expected_cell.1),
            "cursor for prompt position {} and cursor position {}",
            position(prompt, "", columns),
            position(prompt, &line[..cursor], columns)
        );
    }

    #[test]
    fn position_counts_display_columns() {
        assert_eq!(position("", "abc", 80), 3);
        assert_eq!(position("", "root@xila:/$ ", 80), 13);
        assert_eq!(position("", "\u{754C}\u{304B}\u{306A}", 80), 6);
        assert_eq!(position("", "\u{FF41}\u{FF42}", 80), 4);
        assert_eq!(position("", "e\u{301}", 80), 1);
        assert_eq!(position("", "\u{1F600}", 80), 2);
        assert_eq!(position("", "\x1b[31mred\x1b[0m", 80), 3);
        assert_eq!(position("", "a\tb", 80), 2);
        // A double width character that does not fit wraps and leaves a cell
        // empty: 79 cells plus a wide character take 82 cells.
        assert_eq!(position("", &("x".repeat(79) + "\u{754C}"), 80), 82);
    }

    #[test]
    fn redraw_moves_relative_to_the_cursor() {
        let mut out = String::new();
        redraw(&mut out, "ab$ ", position("ab$ ", "he", 80), "hello", 2, 80);
        assert_eq!(out, "\r\x1b[4C\x1b[Jhello\r\x1b[4Che");

        let line = "x".repeat(100);
        let mut out = String::new();
        redraw(
            &mut out,
            "ab$ ",
            position("ab$ ", &line, 80),
            &line,
            100,
            80,
        );
        assert_eq!(out, "\r\x1b[1A\x1b[4C\x1b[J".to_string() + &line);

        let mut out = String::new();
        erase_line(&mut out, "ab$ ", position("ab$ ", "he", 80), 80);
        assert_eq!(out, "\r\x1b[4C\x1b[J");

        // A cursor left pending at the end of a row is moved explicitly when
        // printing stopped exactly at the terminal width: the carriage return
        // and line feed land on the next row, scrolling at the bottom row.
        let line = "x".repeat(76);
        let mut out = String::new();
        redraw(&mut out, "ab$ ", position("ab$ ", &line, 80), &line, 76, 80);
        assert_eq!(out, "\r\x1b[1A\x1b[4C\x1b[J".to_string() + &line + "\r\n");
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
        feed_prompt(&mut model, prompt, columns);
        let line = "x".repeat(100);
        let mut from_position = position(prompt, "", columns);
        for length in 0..=line.len() {
            let mut stream = String::new();
            redraw(
                &mut stream,
                prompt,
                from_position,
                &line[..length],
                length,
                columns,
            );
            model.feed(&stream);
            from_position = position(prompt, &line[..length], columns);
        }
        check_screen_and_cursor(&model, prompt, &line, line.len(), columns);
        for cursor in [50, 0] {
            let mut stream = String::new();
            redraw(&mut stream, prompt, from_position, &line, cursor, columns);
            model.feed(&stream);
            from_position = position(prompt, &line[..cursor], columns);
            check_screen_and_cursor(&model, prompt, &line, cursor, columns);
        }
    }

    #[test]
    fn wrapped_redraw_from_the_bottom_row_at_80_columns() {
        check_wrapped_redraw(80);
    }

    #[test]
    fn wrapped_redraw_from_the_bottom_row_at_40_columns() {
        check_wrapped_redraw(40);
    }

    /// Wide characters wrap at display columns, not at scalar values:
    /// 35 CJK characters after a 13 column prompt take 70 columns and wrap
    /// to the next row, while 50 letters plus a combining mark stay on the
    /// prompt row.
    #[test]
    fn wrapped_redraw_with_wide_and_combining_characters() {
        let columns = 80;
        let height = 6;
        for line in ["\u{754C}".repeat(35), "e".repeat(50) + "\u{301}"] {
            let mut model = Model::new(columns, height);
            for _ in 1..height {
                model.feed("output line\r\n");
            }
            let prompt = "root@xila:/$ ";
            feed_prompt(&mut model, prompt, columns);
            let mut from_position = position(prompt, "", columns);
            for length in 1..=line.chars().count() {
                let cursor = line.char_indices().nth(length - 1).map_or(0, |(index, _)| {
                    index + line[index..].chars().next().map_or(0, char::len_utf8)
                });
                let text = &line[..cursor];
                let mut stream = String::new();
                redraw(&mut stream, prompt, from_position, text, cursor, columns);
                model.feed(&stream);
                from_position = position(prompt, text, columns);
                check_screen_and_cursor(&model, prompt, text, cursor, columns);
            }
            // Move the cursor to the start of the line and check again.
            let mut stream = String::new();
            redraw(&mut stream, prompt, from_position, &line, 0, columns);
            model.feed(&stream);
            check_screen_and_cursor(&model, prompt, &line, 0, columns);
        }
    }

    /// Prompts that fill a row exactly (or one column more) must not be
    /// overwritten by the redrawn line, and lines that end exactly at the
    /// terminal width must place the cursor correctly.
    #[test]
    fn redraw_with_prompts_that_fill_the_row() {
        let columns = 80;
        for prompt_columns in [79, 80, 81] {
            for total in [80, 81, 160] {
                if total <= prompt_columns {
                    continue;
                }
                let line_length = total - prompt_columns;
                let prompt = "p".repeat(prompt_columns);
                let line = "x".repeat(line_length);
                let height = 8;
                let mut model = Model::new(columns, height);
                model.feed("filler\r\nfiller\r\n");
                feed_prompt(&mut model, &prompt, columns);
                let mut from_position = position(&prompt, "", columns);
                for length in 1..=line_length {
                    let mut stream = String::new();
                    redraw(
                        &mut stream,
                        &prompt,
                        from_position,
                        &line[..length],
                        length,
                        columns,
                    );
                    model.feed(&stream);
                    from_position = position(&prompt, &line[..length], columns);
                    check_screen_and_cursor(&model, &prompt, &line[..length], length, columns);
                }
                // Home, then type one more character in front of the line.
                let mut stream = String::new();
                redraw(&mut stream, &prompt, from_position, &line, 0, columns);
                model.feed(&stream);
                check_screen_and_cursor(&model, &prompt, &line, 0, columns);
                let extended = format!("y{line}");
                let mut stream = String::new();
                redraw(
                    &mut stream,
                    &prompt,
                    position(&prompt, "", columns),
                    &extended,
                    1,
                    columns,
                );
                model.feed(&stream);
                check_screen_and_cursor(&model, &prompt, &extended, 1, columns);
            }
        }
    }

    #[test]
    fn cells_distinguish_logical_and_physical_positions() {
        assert_eq!(cell(0, 80), (0, 0));
        assert_eq!(cell(79, 80), (0, 79));
        assert_eq!(cell(80, 80), (1, 0));
        assert_eq!(cell(81, 80), (1, 1));
        assert_eq!(cell(160, 80), (2, 0));
        assert_eq!(physical_cell(0, 80), (0, 0));
        assert_eq!(physical_cell(79, 80), (0, 79));
        assert_eq!(physical_cell(80, 80), (0, 79));
        assert_eq!(physical_cell(81, 80), (1, 1));
        assert_eq!(physical_cell(160, 80), (1, 79));
    }
}
