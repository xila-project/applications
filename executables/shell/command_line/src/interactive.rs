use crate::{
    OutputBuffer, Result, Shell, execution,
    line_editor::{Action, DEFAULT_HISTORY_CAPACITY, LineEditor},
};
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
                Action::Submit => {
                    if let Some(source) = editor.finish() {
                        if let Err(error) = self.execute_line(&source).await {
                            self.last_status = 2;
                            self.diagnostic(error).await;
                        }
                    } else {
                        self.last_status = 2;
                        self.diagnostic(xila::internationalization::t!(
                            "execution.errors.command_line_too_long"
                        ))
                        .await;
                    }
                }
                Action::Cancel => editor.discard(),
                _ => self.running = false,
            }
        }
        Ok(())
    }

    async fn edit_command(&mut self, editor: &mut LineEditor) -> Result<Action> {
        let mut output = OutputBuffer::default();
        output
            .write(
                self.standard.out(),
                format_args!(
                    "{}@{}:{}$ \x1b7",
                    self.user, self.host, self.current_directory
                ),
            )
            .await?;
        let _ = self.standard.out().flush().await;
        loop {
            let mut byte = [0; 1];
            let action = match self.read_input_byte(&mut byte).await? {
                Some(0) if !editor.line().is_empty() => Action::Submit,
                Some(1) => editor.feed(byte[0]),
                _ => Action::EndOfInput,
            };
            match action {
                Action::None => continue,
                Action::Redraw => {
                    // Restore the position after the prompt. Erase any old text, including
                    // wrapped rows. Replay the prefix to place the cursor inside the line.
                    let prefix = if editor.before_cursor().len() == editor.line().len() {
                        ""
                    } else {
                        "\x1b8"
                    };
                    let before_cursor = if prefix.is_empty() {
                        ""
                    } else {
                        editor.before_cursor()
                    };
                    output
                        .write(
                            self.standard.out(),
                            format_args!("\x1b8\x1b[J{}{prefix}{before_cursor}", editor.line()),
                        )
                        .await?;
                }
                Action::Submit => {
                    output
                        .write(
                            self.standard.out(),
                            format_args!("\x1b8{}\n", editor.line()),
                        )
                        .await?;
                    return Ok(action);
                }
                Action::Cancel | Action::EndOfInput => {
                    execution::write_all(self.standard.out(), b"\x1b8\x1b[J\n").await?;
                    return Ok(action);
                }
                Action::Bell => execution::write_all(self.standard.out(), b"\x07").await?,
            }
            let _ = self.standard.out().flush().await;
        }
    }
}
