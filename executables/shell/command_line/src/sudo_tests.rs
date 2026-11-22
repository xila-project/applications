use super::*;
use alloc::{format, vec};

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_string()).collect()
}

#[test]
fn parsing_defaults_to_root_and_keeps_the_command_literal() {
    let arguments = strings(&["package", "install", "-u", "x"]);
    assert_eq!(
        Request::parse(&arguments).unwrap(),
        Request {
            target: "root",
            command: &arguments[..],
        }
    );
}

#[test]
fn parsing_reads_the_target_user_and_the_end_of_options() {
    let arguments = strings(&["-u", "guest", "--", "-command"]);
    assert_eq!(
        Request::parse(&arguments).unwrap(),
        Request {
            target: "guest",
            command: &arguments[3..],
        }
    );
    let arguments = strings(&["--user", "guest", "identify"]);
    assert_eq!(Request::parse(&arguments).unwrap().target, "guest");
}

#[test]
fn parsing_rejects_a_missing_command_user_or_unknown_option() {
    for arguments in [
        strings(&[]),
        strings(&["-u"]),
        strings(&["-u", "guest"]),
        strings(&["-u", "", "identify"]),
        strings(&["--"]),
        strings(&["-s", "identify"]),
    ] {
        assert!(
            matches!(Request::parse(&arguments), Err(Error::SudoUsage)),
            "{arguments:?}"
        );
    }
}

const USER: UserIdentifier = UserIdentifier::new(2000);
const OTHER: UserIdentifier = UserIdentifier::new(2001);

fn seconds(value: u64) -> Duration {
    Duration::from_secs(value)
}

#[test]
fn the_lockout_starts_after_three_failures_and_doubles_up_to_five_minutes() {
    assert_eq!(lockout_after(0), None);
    assert_eq!(lockout_after(2), None);
    assert_eq!(lockout_after(3), Some(seconds(30)));
    assert_eq!(lockout_after(4), Some(seconds(60)));
    assert_eq!(lockout_after(5), Some(seconds(120)));
    assert_eq!(lockout_after(6), Some(seconds(240)));
    assert_eq!(lockout_after(7), Some(seconds(300)));
    assert_eq!(lockout_after(40), Some(seconds(300)));
    assert_eq!(lockout_after(u32::MAX), Some(seconds(300)));
}

/// One failed attempt of `user` at `now`, run to its end.
fn fail(limiter: &mut RateLimiter, user: UserIdentifier, now: Duration) {
    assert_eq!(limiter.admit(user, now), Ok(()));
    limiter.record_failure(user, now);
    limiter.finish(user);
}

#[test]
fn the_rate_limiter_locks_out_one_user_until_the_delay_ends() {
    let mut limiter = RateLimiter::new();
    fail(&mut limiter, USER, seconds(100));
    fail(&mut limiter, USER, seconds(101));
    fail(&mut limiter, USER, seconds(102));
    assert_eq!(
        limiter.admit(USER, seconds(102)),
        Err(Refusal::LockedOut(seconds(30)))
    );
    assert_eq!(
        limiter.admit(USER, seconds(122)),
        Err(Refusal::LockedOut(seconds(10)))
    );
    // Other users are not affected.
    assert_eq!(limiter.admit(OTHER, seconds(102)), Ok(()));
    limiter.finish(OTHER);

    // One more failure after the lockout doubles it.
    fail(&mut limiter, USER, seconds(132));
    assert_eq!(
        limiter.admit(USER, seconds(132)),
        Err(Refusal::LockedOut(seconds(60)))
    );

    // A success forgets every failure.
    assert_eq!(limiter.admit(USER, seconds(192)), Ok(()));
    limiter.record_failure(USER, seconds(192));
    limiter.record_success(USER);
    limiter.finish(USER);
    assert!(limiter.users.is_empty());
    fail(&mut limiter, USER, seconds(193));
    assert_eq!(limiter.admit(USER, seconds(193)), Ok(()));
}

#[test]
fn the_rate_limiter_admits_one_attempt_per_user_at_a_time() {
    let mut limiter = RateLimiter::new();
    assert_eq!(limiter.admit(USER, seconds(1)), Ok(()));
    // A concurrent attempt from another Shell is refused, so concurrent
    // prompts cannot all pass before a failure is counted.
    assert_eq!(limiter.admit(USER, seconds(1)), Err(Refusal::Busy));
    assert_eq!(limiter.admit(OTHER, seconds(1)), Ok(()));
    // An attempt that ends without reaching the check (cancelled at the
    // prompt) is not counted.
    limiter.finish(USER);
    assert_eq!(limiter.admit(USER, seconds(2)), Ok(()));
    // An attempt interrupted during the check stays counted: it was counted
    // before the check and only a success clears it.
    limiter.record_failure(USER, seconds(2));
    limiter.finish(USER);
    assert_eq!(limiter.users[&USER].failures, 1);
}

#[test]
fn concurrent_attempts_cannot_pass_the_threshold() {
    let mut limiter = RateLimiter::new();
    fail(&mut limiter, USER, seconds(1));
    fail(&mut limiter, USER, seconds(1));
    // The third attempt is in progress: every concurrent one is refused, and
    // once it fails, the user is locked out.
    assert_eq!(limiter.admit(USER, seconds(1)), Ok(()));
    for _ in 0..10 {
        assert_eq!(limiter.admit(USER, seconds(1)), Err(Refusal::Busy));
    }
    limiter.record_failure(USER, seconds(1));
    limiter.finish(USER);
    assert_eq!(
        limiter.admit(USER, seconds(1)),
        Err(Refusal::LockedOut(seconds(30)))
    );
}

#[test]
fn only_direct_entries_of_the_secure_path_are_allowed() {
    for (name, entry) in [
        ("package", Some("package")),
        ("/applications/executables/package", Some("package")),
        ("./package", None),
        ("executables/package", None),
        ("../applications/executables/package", None),
        ("/applications/executables/../../users/package", None),
        ("/applications/executables/sub/package", None),
        ("/applications/executables/", None),
        ("/applications/executables", None),
        ("/applications/executablesX/package", None),
        ("/users/administrator/package", None),
        (".", None),
        ("..", None),
        ("/applications/executables/..", None),
    ] {
        assert_eq!(secure_entry(name), entry, "{name}");
    }
}

#[test]
fn the_password_is_capped_without_reallocating() {
    let mut password = Password::new();
    let pointer = password.0.as_ptr();
    for _ in 0..MAXIMUM_PASSWORD_BYTES {
        assert!(password.push(0xFF));
    }
    assert!(!password.push(b'x'));
    assert_eq!(password.0.len(), MAXIMUM_PASSWORD_BYTES);
    assert_eq!(password.0.as_ptr(), pointer);
    // Not UTF-8, so it cannot match any account.
    assert_eq!(password.as_str(), None);
}

#[test]
fn the_audit_record_names_caller_target_command_and_outcome() {
    let command = strings(&["/applications/executables/package", "install", "a\"b"]);
    let record = AuditRecord {
        caller: "administrator",
        caller_identifier: UserIdentifier::new(1000),
        target: "root",
        command: &command,
        outcome: Outcome::Completed(0),
    };
    assert_eq!(
        format!("{record}"),
        "sudo: caller \"administrator\" (1000), target \"root\", command \
         \"/applications/executables/package install a\\\"b\": ran, exit status 0"
    );
    for (outcome, text) in [
        (Outcome::NotAllowed, "refused, not an administrator"),
        (Outcome::WrongPassword, "refused, wrong password"),
        (
            Outcome::LockedOut(seconds(30)),
            "refused, locked out for 30 more seconds",
        ),
        (Outcome::Aborted, "aborted during the password prompt"),
    ] {
        let record = AuditRecord { outcome, ..record };
        assert!(format!("{record}").ends_with(text), "{outcome:?}");
    }
}

#[test]
fn the_audit_record_escapes_the_fields_the_caller_chooses() {
    let command = strings(&["identify\nsudo: forged"]);
    let record = AuditRecord {
        caller: "administrator\"",
        caller_identifier: UserIdentifier::new(1000),
        target: "root\n\u{1b}[2Jsudo: caller \"x\"",
        command: &command,
        outcome: Outcome::UnknownTargetUser,
    };
    let text = format!("{record}");
    assert!(!text.contains('\n'));
    assert!(!text.contains('\u{1b}'));
    assert_eq!(
        text,
        "sudo: caller \"administrator\\\"\" (1000), target \"root\\n\\u{1b}[2Jsudo: caller \\\"x\\\"\", \
         command \"identify\\nsudo: forged\": refused, unknown target user"
    );
}

#[test]
fn wiping_overwrites_the_whole_allocation() {
    let mut bytes = Vec::with_capacity(MAXIMUM_PASSWORD_BYTES);
    bytes.extend_from_slice(b"a longer secret");
    bytes.truncate(3);
    let capacity = bytes.capacity();
    wipe(&mut bytes);
    assert!(bytes.is_empty());
    assert_eq!(bytes.capacity(), capacity);
    // SAFETY: the allocation is still owned by `bytes` and every byte of it
    // was just written.
    let written = unsafe { core::slice::from_raw_parts(bytes.as_ptr(), capacity) };
    assert_eq!(written, vec![0; capacity].as_slice());
}
