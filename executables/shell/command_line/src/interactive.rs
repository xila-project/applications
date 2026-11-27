use crate::{
    OutputBuffer, Result, Shell,
    completion::Matches,
    execution,
    line_editor::{Action, DEFAULT_HISTORY_CAPACITY, LineEditor, Rejection},
    screen,
};
use alloc::string::String;
use xila::file_system::character_device::{
    GET_TERMINAL_ATTRIBUTES, IS_A_TERMINAL, SET_TERMINAL_ATTRIBUTES, TerminalAttributes,
};

const HISTORY_CAPACITY_VARIABLE: &str = "XILA_SHELL_HISTORY_CAPACITY";

impl Shell {
    pub(super) async fn main_interactive(&mut self) -> Result<()> {
        if self.standard.out().control(IS_A_TERMINAL, &()).await != Ok(1) {
            return self.main_stream().await;
        }
        let Ok(saved) = self
            .standard
            .input()
            .control(GET_TERMINAL_ATTRIBUTES, &())
            .await
        else {
            return self.main_stream().await;
        };
        let capacity = self
            .context
            .task_manager
            .get_environment_variable(self.task, HISTORY_CAPACITY_VARIABLE)
            .await
            .ok()
            .and_then(|value| value.get_value().parse().ok())
            .unwrap_or(DEFAULT_HISTORY_CAPACITY);
        let mut editor = LineEditor::new(capacity);
        while self.running {
            if self
                .standard
                .input()
                .control(SET_TERMINAL_ATTRIBUTES, &TerminalAttributes::RAW)
                .await
                .is_err()
            {
                return self.main_stream().await;
            }
            let result = self.edit_command(&mut editor).await;
            self.restore_terminal_attributes(saved).await;
            match result? {
                Action::Submit => match editor.finish() {
                    Ok(source) => {
                        if let Err(error) = self.execute_line(&source).await {
                            self.last_status = 2;
                            self.diagnostic(error).await;
                        }
                    }
                    Err(Rejection::TooLong) => {
                        self.last_status = 2;
                        self.diagnostic(xila::internationalization::t!(
                            "execution.errors.command_line_too_long"
                        ))
                        .await;
                    }
                    Err(Rejection::InvalidUtf8) => {
                        self.last_status = 2;
                        self.diagnostic(xila::internationalization::t!(
                            "errors.invalid_utf_8_input"
                        ))
                        .await;
                    }
                },
                Action::Cancel => editor.discard(),
                _ => self.running = false,
            }
        }
        Ok(())
    }

    async fn edit_command(&mut self, editor: &mut LineEditor) -> Result<Action> {
        let mut output = OutputBuffer::default();
        let prompt = alloc::format!("{}@{}:{}$ ", self.user, self.host, self.current_directory);
        // No device control reports the terminal size yet, so the redraw
        // assumes 80 columns.
        let columns = screen::ASSUMED_TERMINAL_COLUMNS;
        // ESC 7 marks the start of the edited line for the graphical terminal.
        // On a serial console it only saves the cursor position, which the
        // redraw never restores. When the prompt fills a row exactly, the
        // cursor is pending in the last column: move it to the next row, so
        // the redrawn line never overwrites the prompt. Prompt write errors
        // stay non-fatal, as in stream mode: the input read reports a broken
        // terminal instead.
        let mut sequence = String::new();
        sequence.push_str(&prompt);
        sequence.push_str("\x1b7");
        screen::leave_pending_wrap(
            &mut sequence,
            screen::position(&prompt, "", columns),
            columns,
        );
        let _ = output
            .write(self.standard.out(), format_args!("{sequence}"))
            .await;
        let _ = self.standard.out().flush().await;
        let mut pending_matches: Option<Matches> = None;
        loop {
            let mut byte = [0; 1];
            // The position of the cursor in the line shown on the screen,
            // before this key: the redraw moves relative to it.
            let mut displayed_position =
                screen::position(&prompt, editor.before_cursor(), columns);
            let action = match self.read_input_byte(&mut byte).await? {
                Some(0) if !editor.line().is_empty() => Action::Submit,
                Some(1) => editor.feed(byte[0]),
                _ => Action::EndOfInput,
            };
            let action = match action {
                Action::Complete => {
                    if let Some(matches) = pending_matches.take() {
                        self.list_completions(
                            &mut output,
                            &prompt,
                            displayed_position,
                            columns,
                            editor,
                            &matches,
                        )
                        .await;
                        // The prompt was just reprinted below the listing, so
                        // the cursor is back at the start of the line.
                        displayed_position = screen::position(&prompt, "", columns);
                        Action::Redraw
                    } else if let Some(completion) = self
                        .complete(editor.line(), editor.before_cursor().len())
                        .await
                    {
                        if completion.matches.is_ambiguous() {
                            pending_matches = Some(completion.matches);
                        }
                        match completion.replacement {
                            Some((range, text)) => editor.replace(range, &text),
                            // Zero matches ring the bell and leave the line as typed.
                            None if completion.matches.is_empty() => Action::Bell,
                            None => Action::None,
                        }
                    } else {
                        Action::None
                    }
                }
                _ => {
                    pending_matches = None;
                    action
                }
            };

            match action {
                Action::None | Action::Complete => continue,
                Action::Redraw => {
                    let mut sequence = String::new();
                    screen::redraw(
                        &mut sequence,
                        &prompt,
                        displayed_position,
                        editor.line(),
                        editor.before_cursor().len(),
                        columns,
                    );
                    let _ = output
                        .write(self.standard.out(), format_args!("{sequence}"))
                        .await;
                }
                Action::Submit => {
                    let mut sequence = String::new();
                    screen::redraw(
                        &mut sequence,
                        &prompt,
                        displayed_position,
                        editor.line(),
                        editor.line().len(),
                        columns,
                    );
                    // When the line ends exactly at the terminal width, the
                    // redraw already moved the cursor to the start of a new
                    // row; an extra line feed would leave a blank row.
                    if !screen::redraw_ends_on_new_row(&prompt, editor.line(), columns) {
                        sequence.push_str("\r\n");
                    }
                    let _ = output
                        .write(self.standard.out(), format_args!("{sequence}"))
                        .await;
                    return Ok(action);
                }
                Action::Cancel | Action::EndOfInput => {
                    let mut sequence = String::new();
                    screen::erase_line(&mut sequence, &prompt, displayed_position, columns);
                    sequence.push_str("\r\n");
                    let _ = execution::write_all(self.standard.out(), sequence.as_bytes()).await;
                    return Ok(action);
                }
                Action::Bell => {
                    let _ = execution::write_all(self.standard.out(), b"\x07").await;
                }
            }
            let _ = self.standard.out().flush().await;
        }
    }

    /// Echo the line on its own row, list the matches below it and reprint
    /// the prompt for the line that follows the listing.
    async fn list_completions(
        &mut self,
        output: &mut OutputBuffer,
        prompt: &str,
        displayed_position: usize,
        columns: usize,
        editor: &LineEditor,
        matches: &Matches,
    ) {
        let file = self.standard.out();
        let mut sequence = String::new();
        screen::redraw(
            &mut sequence,
            prompt,
            displayed_position,
            editor.line(),
            editor.line().len(),
            columns,
        );
        sequence.push_str("\r\n");
        let _ = output.write(file, format_args!("{sequence}")).await;
        for name in matches.visible() {
            let _ = output.write(file, format_args!("{name}\r\n")).await;
        }
        let hidden = matches.hidden_count();
        if hidden > 0 {
            let _ = output
                .write(
                    file,
                    format_args!(
                        "{}\r\n",
                        xila::internationalization::t!(
                            "completion.listing.hidden_matches",
                            arg0 = hidden
                        )
                    ),
                )
                .await;
        }
        if matches.is_incomplete() {
            let _ = output
                .write(
                    file,
                    format_args!(
                        "{}\r\n",
                        xila::internationalization::t!("completion.listing.search_limit_reached")
                    ),
                )
                .await;
        }
        // Reprint the prompt, moving out of a pending wrap when it fills a
        // row exactly, so the redrawn line never overwrites it.
        let mut sequence = String::new();
        sequence.push_str(prompt);
        sequence.push_str("\x1b7");
        screen::leave_pending_wrap(
            &mut sequence,
            screen::position(prompt, "", columns),
            columns,
        );
        let _ = output.write(file, format_args!("{sequence}")).await;
    }
}
