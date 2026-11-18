use crate::{Result, Shell, error::Error};
use alloc::string::String;
use core::fmt::Write;
use xila::{
    authentication,
    file_system::character_device::{GET_TERMINAL_ATTRIBUTES, SET_TERMINAL_ATTRIBUTES},
    internationalization::t,
};

impl Shell {
    // Unlike Standard::read_line, a login prompt must stop at EOF rather than
    // repeatedly retrying an already closed stdin (e.g. docker run without -i).
    async fn read_login_line(&mut self, buffer: &mut String) -> Result<bool> {
        buffer.clear();
        let mut byte = [0u8; 1];
        loop {
            let Some(count) = self.read_input_byte(&mut byte).await? else {
                return Ok(false);
            };
            if count == 0 {
                return Ok(false);
            }
            if byte[0] == b'\n' {
                return Ok(true);
            }
            if buffer.len() >= 256 {
                return Err(Error::LineTooLong);
            }
            buffer.push(char::from(byte[0]));
        }
    }

    /// Read the password with terminal echo turned off.
    ///
    /// The previous terminal attributes are restored whatever the outcome of
    /// the read. When standard input does not support terminal attributes
    /// (a host console, a pipe or a file), the password is read as before,
    /// without any message. With echo off, the terminal still writes the line
    /// break on Enter, so the prompt is left the same way in both cases.
    async fn read_password_line(&mut self, buffer: &mut String) -> Result<bool> {
        let input = self.standard.input();
        let mut restore = None;
        if let Ok(attributes) = input.control(GET_TERMINAL_ATTRIBUTES, &()).await
            && attributes.is_echo_enabled()
            && input
                .control(SET_TERMINAL_ATTRIBUTES, &attributes.with_echo(false))
                .await
                .is_ok()
        {
            restore = Some(attributes);
        }

        let result = self.read_login_line(buffer).await;

        if let Some(attributes) = restore {
            let _ = self
                .standard
                .input()
                .control(SET_TERMINAL_ATTRIBUTES, &attributes)
                .await;
        }
        result
    }

    pub async fn authenticate(&mut self) -> Result<Option<String>> {
        write!(
            self.standard.out(),
            "{}",
            t!("authentication.prompts.username")
        )?;
        let _ = self.standard.out().flush().await;

        let mut user_name = String::with_capacity(32);
        if !self.read_login_line(&mut user_name).await? {
            return Ok(None);
        }

        write!(
            self.standard.out(),
            "{}",
            t!("authentication.prompts.password")
        )?;
        let _ = self.standard.out().flush().await;

        let mut password = String::with_capacity(32);
        if !self.read_password_line(&mut password).await? {
            return Ok(None);
        }

        // - Check the user name and the password
        let task_manager = &self.context.task_manager;
        let task = task_manager.get_current_task_identifier().await;
        let authentication_context = authentication::Context {
            virtual_file_system: self.context.virtual_file_system,
            task_manager,
            users_manager: self.context.users_manager,
            task,
        };
        let user_identifier =
            authentication::authenticate_user(&authentication_context, &user_name, &password)
                .await
                .map_err(Error::AuthenticationFailed)?;

        let home = self
            .context
            .virtual_file_system
            .ensure_user_home(task, user_identifier)
            .await
            .map_err(Error::FailedToCreateDirectory)?;
        task_manager
            .set_environment_variable(task, "HOME", home.as_str())
            .await
            .map_err(Error::FailedToSetEnvironmentVariable)?;

        // - Set the user
        task_manager
            .set_user(task, user_identifier)
            .await
            .map_err(Error::FailedToSetTaskUser)?;

        task_manager
            .set_environment_variable(task, "User", &user_name)
            .await
            .map_err(Error::FailedToSetEnvironmentVariable)?;

        Ok(Some(user_name))
    }
}
