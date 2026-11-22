use crate::{Result, Shell, error::Error};
use alloc::string::String;
use core::fmt::Write;
use xila::{
    authentication,
    file_system::character_device::{
        GET_TERMINAL_ATTRIBUTES, SET_TERMINAL_ATTRIBUTES, TerminalAttributes,
    },
    internationalization::t,
    virtual_file_system,
};

/// Whether `error` means that standard input has no terminal attributes.
///
/// A device or file that does not implement the control command reports
/// `Unsupported`; a file that is not a terminal at all reports
/// `InappropriateIoControl`.
const fn is_unsupported(error: virtual_file_system::IoErrorKind) -> bool {
    matches!(
        error,
        virtual_file_system::IoErrorKind::Unsupported
            | virtual_file_system::IoErrorKind::InappropriateIoControl
    )
}

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
    /// When standard input does not support terminal attributes (a host
    /// console, a pipe or a file: getting or setting them answers
    /// `UnsupportedOperation`), the password is read as before, without any
    /// message. Any other failure to turn echo off fails the authentication
    /// before the password is read, so it is never echoed by mistake.
    ///
    /// The previous terminal attributes are restored whatever the outcome of
    /// the read; see [`Self::restore_terminal_attributes`]. With echo off, the
    /// terminal still writes the line break on Enter, so the prompt is left the
    /// same way in both cases.
    pub(crate) async fn read_password_line(&mut self, buffer: &mut String) -> Result<bool> {
        let input = self.standard.input();
        let attributes = match input.control(GET_TERMINAL_ATTRIBUTES, &()).await {
            Ok(attributes) => Some(attributes),
            Err(error) if is_unsupported(error) => None,
            Err(error) => return Err(Error::FailedToHidePassword(error)),
        };

        let mut restore = None;
        if let Some(attributes) = attributes
            && attributes.is_echo_enabled()
        {
            match input
                .control(SET_TERMINAL_ATTRIBUTES, &attributes.with_echo(false))
                .await
            {
                Ok(()) => restore = Some(attributes),
                Err(error) if is_unsupported(error) => {}
                Err(error) => return Err(Error::FailedToHidePassword(error)),
            }
        }

        let result = self.read_login_line(buffer).await;

        if let Some(attributes) = restore {
            self.restore_terminal_attributes(attributes).await;
        }
        result
    }

    /// Restore the terminal attributes saved before reading the password.
    ///
    /// A failed restore is retried once. If it still fails, echo stays off
    /// and the error is reported on standard error, without discarding the
    /// password that was read.
    pub(crate) async fn restore_terminal_attributes(&mut self, attributes: TerminalAttributes) {
        let input = self.standard.input();
        if input
            .control(SET_TERMINAL_ATTRIBUTES, &attributes)
            .await
            .is_ok()
        {
            return;
        }
        if let Err(error) = input.control(SET_TERMINAL_ATTRIBUTES, &attributes).await {
            self.diagnostic(Error::FailedToRestoreTerminalEcho(error))
                .await;
        }
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
