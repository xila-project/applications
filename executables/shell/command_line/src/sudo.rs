//! `sudo [-u USER] COMMAND [ARGUMENT]...`: run one external command as another
//! user (`root` by default) after authenticating the calling user again.
//!
//! - Only members of the administrator group may use it. Anyone else is
//!   refused before any prompt.
//! - The caller's own password is read from a terminal whose echo was turned
//!   off and checked to be off; any other standard input is refused before the
//!   prompt. It is never taken from the arguments or the environment.
//! - One attempt per user at a time, across every Shell. Each attempt counts
//!   as a failure before the password is checked and is cleared only by a
//!   success, and consecutive failures lock the caller out for a growing delay.
//! - The command is a direct entry of [`SECURE_PATH`]. It runs in its own task
//!   with the target identity and a fixed, minimal environment that does not
//!   depend on the caller's. The Shell's own task never changes identity.
//! - WebAssembly guests never run under `sudo`: Core marks the command's task
//!   and every task it starts as running with a granted identity
//!   (`task::Manager::is_identity_granted`), a mark nothing clears, and the
//!   Wasm runtime refuses to start a guest in a marked task.
//! - Every attempt is logged with the caller, the target user, the command and
//!   the outcome. The password is never logged.

use crate::{Error, Result, Shell, commands::resolve_user_command, resolver::resolve};
use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use core::{cell::RefCell, fmt, fmt::Write, time::Duration};
use xila::{
    authentication,
    executable::{Launch, execute_with},
    file_system::{
        Kind, Path,
        character_device::{GET_TERMINAL_ATTRIBUTES, SET_TERMINAL_ATTRIBUTES, TerminalAttributes},
    },
    internationalization::t,
    synchronization::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    task::EnvironmentVariable,
    users::{ADMINISTRATOR_GROUP_NAME, GroupIdentifier, UserIdentifier},
    virtual_file_system::IoErrorKind,
};

/// The only directory that commands run by `sudo` come from. The caller's
/// `Paths` and working directory are not used, so they cannot change which
/// program runs. Only root can add entries to it.
pub(crate) const SECURE_PATH: &str = "/applications/executables";

/// Environment variable that tells a command run by `sudo` the name of the
/// user who ran it. It is information only: the command can change it, so no
/// check relies on it (the Wasm runtime uses Core's granted identity mark).
pub const SUDO_USER_VARIABLE: &str = "Sudo_user";

/// `Host` of a command run by `sudo`, instead of the caller's, which the
/// caller can change.
pub(crate) const SUDO_HOST: &str = "xila";

/// User that a command runs as without `-u`.
const DEFAULT_TARGET_USER: &str = "root";

/// Longest password accepted, in bytes. A longer line is read to its end
/// with echo still off, then refused.
pub(crate) const MAXIMUM_PASSWORD_BYTES: usize = 256;

/// Failed attempts in a row allowed before the caller is locked out.
const FAILURES_BEFORE_LOCKOUT: u32 = 3;
/// Lockout after [`FAILURES_BEFORE_LOCKOUT`] failures; doubled by each further
/// failure, up to [`MAXIMUM_LOCKOUT`].
const FIRST_LOCKOUT: Duration = Duration::from_secs(30);
const MAXIMUM_LOCKOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct UserState {
    /// Consecutive failures, including the attempt in progress once its
    /// password was read.
    failures: u32,
    last_failure: Duration,
    attempt_in_progress: bool,
}

/// Why an attempt may not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    LockedOut(Duration),
    /// Another attempt of the same user is in progress, in this Shell or
    /// another one.
    Busy,
}

/// Authentication attempts per calling user.
#[derive(Debug, Default)]
pub(crate) struct RateLimiter {
    users: BTreeMap<UserIdentifier, UserState>,
}

impl RateLimiter {
    pub(crate) const fn new() -> Self {
        Self {
            users: BTreeMap::new(),
        }
    }

    /// Start an attempt of `user`, unless one is in progress or `user` is
    /// locked out. Checking and starting are one step, so concurrent Shells
    /// cannot both start.
    pub(crate) fn admit(
        &mut self,
        user: UserIdentifier,
        now: Duration,
    ) -> core::result::Result<(), Refusal> {
        let state = self.users.entry(user).or_default();
        if state.attempt_in_progress {
            return Err(Refusal::Busy);
        }
        if let Some(lockout) = lockout_after(state.failures) {
            let end = state.last_failure.saturating_add(lockout);
            if now < end {
                return Err(Refusal::LockedOut(end - now));
            }
        }
        state.attempt_in_progress = true;
        Ok(())
    }

    /// Count the attempt in progress as failed. Called before the password is
    /// checked, so that interrupting the check cannot hide a failure.
    pub(crate) fn record_failure(&mut self, user: UserIdentifier, now: Duration) {
        let state = self.users.entry(user).or_default();
        state.failures = state.failures.saturating_add(1);
        state.last_failure = now;
    }

    /// Forget every failure of `user`.
    pub(crate) fn record_success(&mut self, user: UserIdentifier) {
        if let Some(state) = self.users.get_mut(&user) {
            state.failures = 0;
        }
    }

    /// End the attempt in progress of `user`.
    pub(crate) fn finish(&mut self, user: UserIdentifier) {
        if let Some(state) = self.users.get_mut(&user) {
            state.attempt_in_progress = false;
            if state.failures == 0 {
                self.users.remove(&user);
            }
        }
    }
}

/// The lockout that follows `failures` consecutive failures.
fn lockout_after(failures: u32) -> Option<Duration> {
    let doublings = failures.checked_sub(FAILURES_BEFORE_LOCKOUT)?;
    let factor = 1_u32.checked_shl(doublings).unwrap_or(u32::MAX);
    Some(
        FIRST_LOCKOUT
            .checked_mul(factor)
            .unwrap_or(MAXIMUM_LOCKOUT)
            .min(MAXIMUM_LOCKOUT),
    )
}

/// Shared by every Shell, so opening another terminal does not reset it.
static RATE_LIMITER: Mutex<CriticalSectionRawMutex, RefCell<RateLimiter>> =
    Mutex::new(RefCell::new(RateLimiter::new()));

/// An attempt admitted by [`RATE_LIMITER`], ended when dropped, including
/// when the Shell is stopped in the middle of it.
struct Attempt {
    user: UserIdentifier,
}

impl Attempt {
    fn start(user: UserIdentifier, now: Duration) -> core::result::Result<Self, Refusal> {
        RATE_LIMITER.lock(|limiter| limiter.borrow_mut().admit(user, now))?;
        Ok(Self { user })
    }

    fn record_failure(&self, now: Duration) {
        RATE_LIMITER.lock(|limiter| limiter.borrow_mut().record_failure(self.user, now));
    }

    fn record_success(&self) {
        RATE_LIMITER.lock(|limiter| limiter.borrow_mut().record_success(self.user));
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        RATE_LIMITER.lock(|limiter| limiter.borrow_mut().finish(self.user));
    }
}

/// The parsed command line of `sudo`.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request<'a> {
    pub(crate) target: &'a str,
    pub(crate) command: &'a [String],
}

impl<'a> Request<'a> {
    /// Parse the arguments that follow `sudo`.
    pub(crate) fn parse(arguments: &'a [String]) -> Result<Self> {
        let mut target = DEFAULT_TARGET_USER;
        let mut index = 0;
        while let Some(argument) = arguments.get(index) {
            match argument.as_str() {
                "-u" | "--user" => {
                    target = arguments.get(index + 1).ok_or(Error::SudoUsage)?;
                    index += 2;
                }
                "--" => {
                    index += 1;
                    break;
                }
                option if option.starts_with('-') => return Err(Error::SudoUsage),
                _ => break,
            }
        }
        let command = &arguments[index..];
        if command.is_empty() || target.is_empty() {
            return Err(Error::SudoUsage);
        }
        Ok(Self { target, command })
    }
}

/// The program that `name` designates: a direct entry of [`SECURE_PATH`],
/// given by name or by absolute path. Anything else is refused, so neither
/// the caller's working directory nor `..` components can lead elsewhere.
pub(crate) fn secure_entry(name: &str) -> Option<&str> {
    let entry = match name.strip_prefix(SECURE_PATH) {
        Some(rest) => rest.strip_prefix('/')?,
        None if name.contains('/') => return None,
        None => name,
    };
    (!entry.is_empty() && !entry.contains('/') && entry != "." && entry != "..").then_some(entry)
}

/// Result of one `sudo` attempt, as written to the system log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    InvalidUsage,
    NotAllowed,
    UnknownTargetUser,
    BuiltIn,
    CommandNotAllowed,
    CommandNotFound,
    LockedOut(Duration),
    Busy,
    EchoNotSuppressed,
    Aborted,
    PasswordTooLong,
    WrongPassword,
    AuthenticationError,
    FailedToStart,
    Completed(usize),
}

/// One line of the audit log. It never holds the password. Every field that
/// the caller can choose is escaped.
pub(crate) struct AuditRecord<'a> {
    pub(crate) caller: &'a str,
    pub(crate) caller_identifier: UserIdentifier,
    pub(crate) target: &'a str,
    pub(crate) command: &'a [String],
    pub(crate) outcome: Outcome,
}

impl fmt::Display for AuditRecord<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "sudo: caller \"{}\" ({}), target \"{}\", command \"",
            self.caller.escape_debug(),
            self.caller_identifier.as_u16(),
            self.target.escape_debug()
        )?;
        for (index, argument) in self.command.iter().enumerate() {
            if index > 0 {
                formatter.write_str(" ")?;
            }
            write!(formatter, "{}", argument.escape_debug())?;
        }
        formatter.write_str("\": ")?;
        match self.outcome {
            Outcome::InvalidUsage => formatter.write_str("refused, invalid usage"),
            Outcome::NotAllowed => formatter.write_str("refused, not an administrator"),
            Outcome::UnknownTargetUser => formatter.write_str("refused, unknown target user"),
            Outcome::BuiltIn => formatter.write_str("refused, built-in command"),
            Outcome::CommandNotAllowed => {
                formatter.write_str("refused, command outside the secure path")
            }
            Outcome::CommandNotFound => formatter.write_str("refused, command not found"),
            Outcome::LockedOut(remaining) => write!(
                formatter,
                "refused, locked out for {} more seconds",
                remaining.as_secs()
            ),
            Outcome::Busy => formatter.write_str("refused, another attempt in progress"),
            Outcome::EchoNotSuppressed => {
                formatter.write_str("refused, standard input cannot hide the password")
            }
            Outcome::Aborted => formatter.write_str("aborted during the password prompt"),
            Outcome::PasswordTooLong => formatter.write_str("refused, password too long"),
            Outcome::WrongPassword => formatter.write_str("refused, wrong password"),
            Outcome::AuthenticationError => formatter.write_str("refused, authentication error"),
            Outcome::FailedToStart => formatter.write_str("failed to start"),
            Outcome::Completed(status) => write!(formatter, "ran, exit status {status}"),
        }
    }
}

/// Write `record` to the system log.
fn audit(
    caller: &str,
    caller_identifier: UserIdentifier,
    target: &str,
    command: &[String],
    outcome: Outcome,
) {
    let record = AuditRecord {
        caller,
        caller_identifier,
        target,
        command,
        outcome,
    };
    match outcome {
        Outcome::Completed(_) => xila::log::information!("{record}"),
        _ => xila::log::warning!("{record}"),
    }
}

/// A password, overwritten when it is dropped.
///
/// Its buffer is allocated once for [`MAXIMUM_PASSWORD_BYTES`] and never
/// grows, so no copy is left behind by a reallocation.
pub(crate) struct Password(Vec<u8>);

impl Password {
    pub(crate) fn new() -> Self {
        Self(Vec::with_capacity(MAXIMUM_PASSWORD_BYTES))
    }

    /// Append `byte`, or return `false` when the password is full.
    pub(crate) fn push(&mut self, byte: u8) -> bool {
        if self.0.len() == MAXIMUM_PASSWORD_BYTES {
            return false;
        }
        self.0.push(byte);
        true
    }

    fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(&self.0).ok()
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Overwrite every byte of `bytes`' allocation with zeros and empty it.
///
/// The whole capacity is overwritten, including bytes left over from a
/// longer value that was cleared.
pub(crate) fn wipe(bytes: &mut Vec<u8>) {
    let capacity = bytes.capacity();
    let pointer = bytes.as_mut_ptr();
    for offset in 0..capacity {
        // SAFETY: `offset` is within the allocation. Volatile writes are not
        // optimized away.
        unsafe { core::ptr::write_volatile(pointer.add(offset), 0) };
    }
    bytes.clear();
}

/// Outcome of checking a password in the authentication task.
enum Verification {
    Accepted(UserIdentifier),
    Rejected(authentication::Error),
    Failed,
}

impl Shell {
    /// Run `sudo` with `arguments`, which start with the `sudo` word itself.
    pub(crate) async fn sudo(&mut self, arguments: &[String]) -> usize {
        match self.sudo_inner(&arguments[1..]).await {
            Ok(status) => status,
            Err(error) => {
                let status = if matches!(error, Error::CommandNotFound) {
                    127
                } else {
                    1
                };
                self.diagnostic(error).await;
                status
            }
        }
    }

    async fn sudo_inner(&mut self, arguments: &[String]) -> Result<usize> {
        let context = self.context;
        let task_manager = context.task_manager;
        let users_manager = context.users_manager;

        let caller = task_manager
            .get_user(self.task)
            .await
            .map_err(Error::FailedToGetTaskUser)?;
        let caller_name = users_manager
            .get_user_name(caller)
            .await
            .map_err(|_| Error::SudoNotAllowed)?;

        let request = match Request::parse(arguments) {
            Ok(request) => request,
            Err(error) => {
                audit(&caller_name, caller, "", arguments, Outcome::InvalidUsage);
                return Err(error);
            }
        };
        let target_name = request.target;
        let command = request.command;
        let audit = |outcome| audit(&caller_name, caller, target_name, command, outcome);

        // - Authorization comes first, so that a refused caller learns nothing
        //   about users or commands and is never prompted.
        if caller != UserIdentifier::ROOT {
            let allowed = match users_manager
                .get_group_identifier(ADMINISTRATOR_GROUP_NAME)
                .await
            {
                Ok(group) => users_manager.is_in_group(caller, group).await,
                Err(_) => false,
            };
            if !allowed {
                audit(Outcome::NotAllowed);
                return Err(Error::SudoNotAllowed);
            }
        }

        // - Target identity.
        let Ok(target) = users_manager.get_user_identifier(target_name).await else {
            audit(Outcome::UnknownTargetUser);
            return Err(Error::SudoUnknownUser(target_name.to_string()));
        };
        let (Ok(target_group), Ok(target_home)) = (
            users_manager.get_user_primary_group(target).await,
            users_manager.get_user_home(target).await,
        ) else {
            audit(Outcome::UnknownTargetUser);
            return Err(Error::SudoUnknownUser(target_name.to_string()));
        };

        // - Command: a direct entry of the secure path, never a built-in.
        let name = &command[0];
        if resolve_user_command(name).is_some() {
            audit(Outcome::BuiltIn);
            return Err(Error::SudoBuiltIn(name.clone()));
        }
        let Some(entry) = secure_entry(name) else {
            audit(Outcome::CommandNotAllowed);
            return Err(Error::SudoCommandNotAllowed(name.clone()));
        };
        let path = match resolve(context, entry, &[Path::from_str(SECURE_PATH)]).await {
            Ok(path) => path,
            Err(error) => {
                audit(Outcome::CommandNotFound);
                return Err(error);
            }
        };

        // - Authentication of the caller (root already has every right).
        if caller != UserIdentifier::ROOT {
            let attempt = match Attempt::start(caller, context.time_manager.monotonic_now()) {
                Ok(attempt) => attempt,
                Err(Refusal::LockedOut(remaining)) => {
                    audit(Outcome::LockedOut(remaining));
                    // Rounded up, so that waiting that long is always enough.
                    let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
                    return Err(Error::SudoLockedOut(seconds));
                }
                Err(Refusal::Busy) => {
                    audit(Outcome::Busy);
                    return Err(Error::SudoBusy);
                }
            };

            let mut password = Password::new();
            if let Err(error) = self.read_sudo_password(&caller_name, &mut password).await {
                audit(match error {
                    Error::SudoEchoNotSuppressed(_) => Outcome::EchoNotSuppressed,
                    Error::SudoPasswordTooLong => Outcome::PasswordTooLong,
                    _ => Outcome::Aborted,
                });
                return Err(error);
            }

            // Counted before the check, and cleared only by a success.
            attempt.record_failure(context.time_manager.monotonic_now());
            match self.verify_password(caller_name.clone(), password).await {
                Verification::Accepted(identifier) if identifier == caller => {
                    attempt.record_success();
                }
                Verification::Rejected(error @ authentication::Error::InvalidPassword) => {
                    audit(Outcome::WrongPassword);
                    return Err(Error::AuthenticationFailed(error));
                }
                Verification::Rejected(error) => {
                    audit(Outcome::AuthenticationError);
                    return Err(Error::AuthenticationFailed(error));
                }
                Verification::Accepted(_) | Verification::Failed => {
                    audit(Outcome::AuthenticationError);
                    return Err(Error::FailedToExecuteCommand);
                }
            }
        }

        // - Execution, in a new task with the target identity and an
        //   environment that does not depend on the caller's.
        let home_is_directory = matches!(
            context.virtual_file_system.get_statistics(&target_home.as_str()).await,
            Ok(statistics) if statistics.kind == Kind::Directory
        );
        let directory = if home_is_directory {
            target_home.as_str()
        } else {
            "/"
        };
        let environment = Vec::from([
            EnvironmentVariable::new("User", target_name),
            EnvironmentVariable::new("HOME", &target_home),
            EnvironmentVariable::new("Host", SUDO_HOST),
            EnvironmentVariable::new("Current_directory", directory),
            EnvironmentVariable::new("Paths", SECURE_PATH),
            EnvironmentVariable::new(SUDO_USER_VARIABLE, &caller_name),
        ]);

        let standard = self
            .standard
            .duplicate()
            .await
            .map_err(Error::FailedToOpenFile)?;
        let launch = Launch::default()
            .with_environment(environment)
            .with_identity(target, target_group);
        let handle = match execute_with(
            context,
            &path,
            command[1..].to_vec(),
            standard,
            None,
            launch,
        )
        .await
        {
            Ok(handle) => handle,
            Err(error) => {
                audit(Outcome::FailedToStart);
                return Err(Error::SudoFailedToExecute(error));
            }
        };
        let status = handle.join().await.unsigned_abs().min(255);
        audit(Outcome::Completed(status));
        Ok(status)
    }

    /// Prompt for the password of `caller_name` and read it into `password`.
    ///
    /// Standard input must be a terminal whose echo can be turned off: the
    /// attributes are read back after the change, and the prompt is written
    /// only once echo is known to be off. Anything else (a pipe, a file, a
    /// console without terminal attributes) is refused before the prompt.
    /// The previous attributes are restored whatever the outcome.
    async fn read_sudo_password(
        &mut self,
        caller_name: &str,
        password: &mut Password,
    ) -> Result<()> {
        let saved = self.suppress_echo().await?;
        let result = self.read_hidden_line(caller_name, password).await;
        if saved.is_echo_enabled() {
            self.restore_terminal_attributes(saved).await;
        }
        result
    }

    /// Turn echo off and check that it is off. Returns the attributes to
    /// restore.
    async fn suppress_echo(&mut self) -> Result<TerminalAttributes> {
        let input = self.standard.input();
        let saved = input
            .control(GET_TERMINAL_ATTRIBUTES, &())
            .await
            .map_err(Error::SudoEchoNotSuppressed)?;
        if saved.is_echo_enabled() {
            input
                .control(SET_TERMINAL_ATTRIBUTES, &saved.with_echo(false))
                .await
                .map_err(Error::SudoEchoNotSuppressed)?;
        }
        let error = match input.control(GET_TERMINAL_ATTRIBUTES, &()).await {
            Ok(current) if !current.is_echo_enabled() => return Ok(saved),
            Ok(_) => IoErrorKind::Unsupported,
            Err(error) => error,
        };
        if saved.is_echo_enabled() {
            self.restore_terminal_attributes(saved).await;
        }
        Err(Error::SudoEchoNotSuppressed(error))
    }

    /// Write the prompt and read one line into `password`, with echo off.
    ///
    /// A line longer than [`MAXIMUM_PASSWORD_BYTES`] is read to its end, so
    /// that none of it reaches the Shell's command reader, and refused. If a
    /// read fails, the rest of the line cannot be discarded, so the Shell
    /// stops instead of reading it as a command once echo is back on.
    async fn read_hidden_line(&mut self, caller_name: &str, password: &mut Password) -> Result<()> {
        write!(
            self.standard.out(),
            "{}",
            t!("sudo.prompts.password", arg0 = caller_name)
        )?;
        let _ = self.standard.out().flush().await;

        let mut byte = [0_u8; 1];
        let mut too_long = false;
        loop {
            let read = self.read_input_byte(&mut byte).await;
            let value = byte[0];
            // SAFETY: `byte` is a live local array.
            unsafe { core::ptr::write_volatile(&mut byte[0], 0) };
            let read = match read {
                Ok(read) => read,
                Err(error) => {
                    self.running = false;
                    return Err(error);
                }
            };
            match read {
                Some(count) if count > 0 => {}
                // The input ended or the task was asked to stop.
                _ if too_long => return Err(Error::SudoPasswordTooLong),
                _ => return Err(Error::SudoAborted),
            }
            if value == b'\n' {
                return if too_long {
                    Err(Error::SudoPasswordTooLong)
                } else {
                    Ok(())
                };
            }
            if !too_long && !password.push(value) {
                too_long = true;
                wipe(&mut password.0);
            }
        }
    }

    /// Check `password` for `user_name`. The password is erased when the
    /// check ends, or when the task cannot be started.
    ///
    /// Account records are readable by root only, so the check runs in a
    /// short-lived task of its own with the root identity; the Shell's task
    /// keeps its identity. That task runs nothing but the check.
    async fn verify_password(&self, user_name: String, password: Password) -> Verification {
        let context = self.context;
        let task_manager = context.task_manager;
        let spawned = task_manager
            .spawn(
                self.task,
                "Shell sudo authentication",
                None,
                move |task| async move {
                    // A password that is not UTF-8 cannot match any account.
                    let Some(text) = password.as_str() else {
                        return Verification::Rejected(authentication::Error::InvalidPassword);
                    };
                    if task_manager
                        .set_user(task, UserIdentifier::ROOT)
                        .await
                        .is_err()
                        || task_manager
                            .set_group(task, GroupIdentifier::ROOT)
                            .await
                            .is_err()
                    {
                        return Verification::Failed;
                    }
                    let authentication_context = authentication::Context {
                        virtual_file_system: context.virtual_file_system,
                        task_manager,
                        users_manager: context.users_manager,
                        task,
                    };
                    match authentication::authenticate_user(
                        &authentication_context,
                        &user_name,
                        text,
                    )
                    .await
                    {
                        Ok(identifier) => Verification::Accepted(identifier),
                        Err(error) => Verification::Rejected(error),
                    }
                },
            )
            .await;
        match spawned {
            Ok((handle, _)) => handle.join().await,
            Err(_) => Verification::Failed,
        }
    }
}

#[cfg(test)]
#[path = "sudo_tests.rs"]
mod tests;
