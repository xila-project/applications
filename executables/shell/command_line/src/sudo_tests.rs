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

#[test]
fn the_rate_limiter_locks_out_one_user_until_the_delay_ends() {
    let mut limiter = RateLimiter::new();
    limiter.record_failure(USER, seconds(100));
    limiter.record_failure(USER, seconds(101));
    assert_eq!(limiter.remaining_lockout(USER, seconds(101)), None);
    limiter.record_failure(USER, seconds(102));
    assert_eq!(
        limiter.remaining_lockout(USER, seconds(102)),
        Some(seconds(30))
    );
    assert_eq!(
        limiter.remaining_lockout(USER, seconds(122)),
        Some(seconds(10))
    );
    assert_eq!(limiter.remaining_lockout(USER, seconds(132)), None);
    // Other users are not affected.
    assert_eq!(limiter.remaining_lockout(OTHER, seconds(102)), None);

    // One more failure after the lockout doubles it.
    limiter.record_failure(USER, seconds(140));
    assert_eq!(
        limiter.remaining_lockout(USER, seconds(140)),
        Some(seconds(60))
    );

    // A success forgets every failure.
    limiter.record_success(USER);
    assert_eq!(limiter.remaining_lockout(USER, seconds(140)), None);
    limiter.record_failure(USER, seconds(150));
    assert_eq!(limiter.remaining_lockout(USER, seconds(150)), None);
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
        "sudo: caller administrator (1000), target root, command \
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
fn wiping_overwrites_the_whole_allocation() {
    let mut password = Password::new();
    assert!(password.0.capacity() >= PASSWORD_CAPACITY);
    password.0.push_str("a longer secret");
    password.0.clear();
    password.0.push_str("secret");
    let mut text = core::mem::take(&mut password.0);
    let capacity = text.capacity();
    wipe(&mut text);
    assert!(text.is_empty());
    assert_eq!(text.capacity(), capacity);
    // SAFETY: the allocation is still owned by `text` and every byte of it
    // was just written.
    let bytes = unsafe { core::slice::from_raw_parts(text.as_ptr(), capacity) };
    assert_eq!(bytes, vec![0; capacity].as_slice());
}
