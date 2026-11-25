//! User-visible text built from the catalog.

use crate::configuration::Invalid;
use crate::error::Error;
use embedded_i18n::t;
use std::string::{String, ToString};

pub fn error_message(error: &Error) -> String {
    match error {
        Error::Configuration => String::from(t!("errors.configuration")),
        Error::MissingToken => String::from(t!("errors.missing_token")),
        Error::Network => String::from(t!("errors.network")),
        Error::Timeout => String::from(t!("errors.timeout")),
        Error::Unauthorized => String::from(t!("errors.unauthorized")),
        Error::Revoked => String::from(t!("errors.revoked")),
        Error::NotFound => String::from(t!("errors.not_found")),
        Error::Http(status) => t!("errors.http", status = status).to_string(),
        Error::InvalidResponse => String::from(t!("errors.invalid_response")),
        Error::TooLarge => String::from(t!("errors.too_large")),
        Error::Storage => String::from(t!("errors.storage")),
    }
}

/// A refused answer of the login flow: the known reasons get their own text,
/// any other reason is shown as it came.
pub fn login_error(reason: &str) -> String {
    match reason {
        "invalid_auth" => String::from(t!("settings.reason.invalid_auth")),
        "invalid_code" => String::from(t!("settings.reason.invalid_code")),
        other => t!("settings.login_failed", reason = other).to_string(),
    }
}

/// Which configuration field is wrong, shown at start.
pub fn invalid_message(invalid: Invalid) -> &'static str {
    match invalid {
        Invalid::Url => t!("invalid.url"),
        Invalid::Refresh => t!("invalid.refresh"),
        Invalid::Entity => t!("invalid.entity"),
        Invalid::TooManyEntities => t!("invalid.too_many_entities"),
    }
}

/// Time since the last refresh, such as `42 s`, `5 min` or `3 h`.
pub fn age(seconds: u64) -> String {
    if seconds < 60 {
        t!("age.seconds", count = seconds).to_string()
    } else if seconds < 3_600 {
        t!("age.minutes", count = seconds / 60).to_string()
    } else {
        t!("age.hours", count = seconds / 3_600).to_string()
    }
}

/// Character shown for each hidden token character.
pub const MASK: char = '*';

/// Mask of `count` characters.
pub fn mask(count: usize) -> String {
    core::iter::repeat_n(MASK, count).collect()
}

/// Applies an edit of a masked field to the secret it hides. The field shows
/// one mask character per secret character, so each mask character stands for
/// the next character of the secret, and any other character is new text,
/// wherever it was typed. Characters deleted are taken from the end: the
/// field cannot tell which of its mask characters went.
pub fn apply_masked_edit(secret: &mut String, shown: &str) {
    let mut old = secret.chars();
    let mut edited = String::new();
    for character in shown.chars() {
        if character == MASK {
            if let Some(kept) = old.next() {
                edited.push(kept);
            }
        } else {
            edited.push(character);
        }
    }
    *secret = edited;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_come_from_the_catalog() {
        assert_eq!(
            error_message(&Error::Unauthorized),
            "Home Assistant refused the token (401 or 403)"
        );
        assert_eq!(error_message(&Error::NotFound), "Entity not found (404)");
        assert_eq!(error_message(&Error::Http(502)), "Home Assistant error 502");
        assert_eq!(
            error_message(&Error::MissingToken),
            "No credentials: sign in or add a token in the settings"
        );
        assert_eq!(
            error_message(&Error::Revoked),
            "Signed out: the device was revoked in Home Assistant"
        );
        assert_eq!(login_error("invalid_auth"), "Wrong username or password");
        assert_eq!(login_error("weird"), "Sign-in failed: weird");
        assert_eq!(age(42), "42 s");
        assert_eq!(age(600), "10 min");
        assert_eq!(age(7_300), "2 h");
        assert_eq!(
            invalid_message(Invalid::Url),
            "The URL must start with https:// or http:// and hold no ?, # or @"
        );
        assert_eq!(
            invalid_message(Invalid::Entity),
            "An entity identifier holds a domain and an object identifier, such as light.living_room"
        );
    }

    #[test]
    fn masked_edits_append_and_delete_at_the_end() {
        let mut token = String::new();
        apply_masked_edit(&mut token, "a");
        apply_masked_edit(&mut token, "*b");
        apply_masked_edit(&mut token, "**cd");
        assert_eq!(token, "abcd");
        assert_eq!(mask(token.chars().count()), "****");
        apply_masked_edit(&mut token, "***");
        assert_eq!(token, "abc");
        apply_masked_edit(&mut token, "");
        assert_eq!(token, "");
        // A pasted value replaces the whole field.
        apply_masked_edit(&mut token, "glsa_pasted");
        assert_eq!(token, "glsa_pasted");
    }
}
