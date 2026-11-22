//! `sudo [-u USER] COMMAND [ARGUMENT]...`: run one external command as another
//! user (`root` by default) after authenticating the calling user again.
//!
//! - Only members of the administrator group may use it. Anyone else is
//!   refused before any prompt.
//! - The caller's own password is read from standard input with echo off. It
//!   is never taken from the arguments or the environment.
//! - Consecutive failed attempts lock the caller out for a growing delay,
//!   shared by every Shell.
//! - The command runs in its own task with the target identity and a minimal
//!   environment. The Shell's own task never changes identity.
//! - Every attempt is logged with the caller, the target user, the command and
//!   the outcome. The password is never logged.

use crate::{Error, Result, Shell, commands::resolve_user_command, resolver::resolve};
use alloc::{
    borrow::ToOwned,
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};
use core::{cell::RefCell, fmt, fmt::Write, time::Duration};
use xila::{
    authentication,
    executable::{Launch, execute_with},
    file_system::{Path, PathOwned},
    internationalization::t,
    synchronization::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    task::EnvironmentVariable,
    users::{ADMINISTRATOR_GROUP_NAME, GroupIdentifier, UserIdentifier},
};

/// The only directory searched for a command given by bare name. The caller's
/// `Paths` is not used, so it cannot change which program runs as root.
pub(crate) const SECURE_PATH: &str = "/applications/executables";

/// User that a command runs as without `-u`.
const DEFAULT_TARGET_USER: &str = "root";

/// Room for the longest password line read, so that the buffer never
/// reallocates and leaves no copy of the password behind. The line reader
/// stops at 256 bytes, but it stores each byte as a character, and the last
/// one may take two bytes.
const PASSWORD_CAPACITY: usize = 256 + 1;

/// Failed attempts in a row allowed before the caller is locked out.
const FAILURES_BEFORE_LOCKOUT: u32 = 3;
/// Lockout after [`FAILURES_BEFORE_LOCKOUT`] failures; doubled by each further
/// failure, up to [`MAXIMUM_LOCKOUT`].
const FIRST_LOCKOUT: Duration = Duration::from_secs(30);
const MAXIMUM_LOCKOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Failures {
    count: u32,
    last: Duration,
}

/// Consecutive authentication failures per calling user.
#[derive(Debug, Default)]
pub(crate) struct RateLimiter {
    failures: BTreeMap<UserIdentifier, Failures>,
}

impl RateLimiter {
    pub(crate) const fn new() -> Self {
        Self {
            failures: BTreeMap::new(),
        }
    }

    /// How long `user` must still wait before the next attempt, if at all.
    pub(crate) fn remaining_lockout(
        &self,
        user: UserIdentifier,
        now: Duration,
    ) -> Option<Duration> {
        let failures = self.failures.get(&user)?;
        let lockout = lockout_after(failures.count)?;
        let end = failures.last.saturating_add(lockout);
        (now < end).then(|| end - now)
    }

    pub(crate) fn record_failure(&mut self, user: UserIdentifier, now: Duration) {
        let failures = self.failures.entry(user).or_insert(Failures {
            count: 0,
            last: now,
        });
        failures.count = failures.count.saturating_add(1);
        failures.last = now;
    }

    pub(crate) fn record_success(&mut self, user: UserIdentifier) {
        self.failures.remove(&user);
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

/// Result of one `sudo` attempt, as written to the system log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    InvalidUsage,
    NotAllowed,
    UnknownTargetUser,
    BuiltIn,
    CommandNotFound,
    LockedOut(Duration),
    Aborted,
    WrongPassword,
    AuthenticationError,
    FailedToStart,
    Completed(usize),
}

/// One line of the audit log. It never holds the password.
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
            "sudo: caller {} ({}), target {}, command \"",
            self.caller,
            self.caller_identifier.as_u16(),
            self.target
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
            Outcome::CommandNotFound => formatter.write_str("refused, command not found"),
            Outcome::LockedOut(remaining) => write!(
                formatter,
                "refused, locked out for {} more seconds",
                remaining.as_secs()
            ),
            Outcome::Aborted => formatter.write_str("aborted during the password prompt"),
            Outcome::WrongPassword => formatter.write_str("refused, wrong password"),
            Outcome::AuthenticationError => formatter.write_str("refused, authentication error"),
            Outcome::FailedToStart => formatter.write_str("failed to start"),
            Outcome::Completed(status) => write!(formatter, "ran, exit status {status}"),
        }
    }
}

/// A password, overwritten when it is dropped.
///
/// The buffer is allocated once with room for the longest line accepted, so
/// reading never reallocates it and leaves no copy behind.
pub(crate) struct Password(String);

impl Password {
    pub(crate) fn new() -> Self {
        Self(String::with_capacity(PASSWORD_CAPACITY))
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

/// Overwrite every byte of `text`'s allocation with zeros and empty it.
///
/// The whole capacity is overwritten, including bytes left over from a
/// longer value that was cleared.
fn wipe(text: &mut String) {
    // SAFETY: only zero bytes are written and the string is then emptied, so
    // it stays valid UTF-8.
    let bytes = unsafe { text.as_mut_vec() };
    let capacity = bytes.capacity();
    let pointer = bytes.as_mut_ptr();
    for offset in 0..capacity {
        // SAFETY: `offset` is within the allocation. Volatile writes are not
        // optimized away.
        unsafe { core::ptr::write_volatile(pointer.add(offset), 0) };
    }
    bytes.clear();
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

        // - Command: external executables only.
        let name = &command[0];
        if resolve_user_command(name).is_some() {
            audit(Outcome::BuiltIn);
            return Err(Error::SudoBuiltIn(name.clone()));
        }
        let path = match self.sudo_resolve(name).await {
            Ok(path) => path,
            Err(error) => {
                audit(Outcome::CommandNotFound);
                return Err(error);
            }
        };

        // - Authentication of the caller (root already has every right).
        if caller != UserIdentifier::ROOT {
            let now = context.time_manager.monotonic_now();
            let remaining =
                RATE_LIMITER.lock(|limiter| limiter.borrow().remaining_lockout(caller, now));
            if let Some(remaining) = remaining {
                audit(Outcome::LockedOut(remaining));
                // Rounded up, so that waiting that long is always enough.
                let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
                return Err(Error::SudoLockedOut(seconds));
            }

            write!(
                self.standard.out(),
                "{}",
                t!("sudo.prompts.password", arg0 = caller_name.as_str())
            )?;
            let _ = self.standard.out().flush().await;

            let mut password = Password::new();
            match self.read_password_line(&mut password.0).await {
                Ok(true) => {}
                Ok(false) => {
                    audit(Outcome::Aborted);
                    return Err(Error::SudoAborted);
                }
                Err(error) => {
                    audit(Outcome::Aborted);
                    return Err(error);
                }
            }

            match self.verify_password(caller_name.clone(), password).await {
                Verification::Accepted(identifier) if identifier == caller => {
                    RATE_LIMITER.lock(|limiter| limiter.borrow_mut().record_success(caller));
                }
                Verification::Rejected(error @ authentication::Error::InvalidPassword) => {
                    let now = context.time_manager.monotonic_now();
                    RATE_LIMITER.lock(|limiter| limiter.borrow_mut().record_failure(caller, now));
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

        // - Execution, in a new task with the target identity and a minimal
        //   environment.
        let mut environment = Vec::with_capacity(5);
        environment.push(EnvironmentVariable::new("User", target_name));
        environment.push(EnvironmentVariable::new("HOME", &target_home));
        if let Ok(host) = task_manager
            .get_environment_variable(self.task, "Host")
            .await
        {
            environment.push(EnvironmentVariable::new("Host", host.get_value()));
        }
        environment.push(EnvironmentVariable::new(
            "Current_directory",
            self.current_directory.as_str(),
        ));
        environment.push(EnvironmentVariable::new("Paths", SECURE_PATH));

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

    /// The program that `name` designates: a path when it contains `/`,
    /// otherwise an entry of [`SECURE_PATH`].
    async fn sudo_resolve(&self, name: &str) -> Result<PathOwned> {
        let path = Path::from_str(name);
        if !name.contains('/') {
            return resolve(self.context, name, &[Path::from_str(SECURE_PATH)]).await;
        }
        if path.is_absolute() {
            return Ok(path.to_owned());
        }
        self.current_directory
            .clone()
            .join(path)
            .ok_or(Error::FailedToJoinPath)
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
                    let result = if task_manager
                        .set_user(task, UserIdentifier::ROOT)
                        .await
                        .is_err()
                        || task_manager
                            .set_group(task, GroupIdentifier::ROOT)
                            .await
                            .is_err()
                    {
                        Verification::Failed
                    } else {
                        let authentication_context = authentication::Context {
                            virtual_file_system: context.virtual_file_system,
                            task_manager,
                            users_manager: context.users_manager,
                            task,
                        };
                        match authentication::authenticate_user(
                            &authentication_context,
                            &user_name,
                            &password.0,
                        )
                        .await
                        {
                            Ok(identifier) => Verification::Accepted(identifier),
                            Err(error) => Verification::Rejected(error),
                        }
                    };
                    drop(password);
                    result
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
