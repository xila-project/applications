//! User-visible text built from the catalog.

use crate::configuration::{Invalid, Kind};
use crate::error::Error;
use crate::format::Unit;
use embedded_i18n::t;
use std::string::{String, ToString};

pub fn error_message(error: &Error) -> String {
    match error {
        Error::Configuration => String::from(t!("errors.configuration")),
        Error::MissingToken => String::from(t!("errors.missing_token")),
        Error::Network => String::from(t!("errors.network")),
        Error::Timeout => String::from(t!("errors.timeout")),
        Error::Unauthorized => String::from(t!("errors.unauthorized")),
        Error::NotFound => String::from(t!("errors.not_found")),
        Error::Http(status) => t!("errors.http", status = status).to_string(),
        Error::InvalidResponse => String::from(t!("errors.invalid_response")),
        Error::TooLarge => String::from(t!("errors.too_large")),
        Error::Storage => String::from(t!("errors.storage")),
        Error::Query(message) => t!("errors.query", message = message).to_string(),
    }
}

pub fn invalid_message(invalid: Invalid) -> &'static str {
    match invalid {
        Invalid::Url => t!("invalid.url"),
        Invalid::DataSource => t!("invalid.data_source"),
        Invalid::Refresh => t!("invalid.refresh"),
        Invalid::Title => t!("invalid.title"),
        Invalid::Expression => t!("invalid.expression"),
        Invalid::Decimals => t!("invalid.decimals"),
        Invalid::Threshold => t!("invalid.threshold"),
        Invalid::Dashboard => t!("invalid.dashboard"),
        Invalid::TooManyPanels => t!("invalid.too_many_panels"),
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

pub fn unit_name(unit: Unit) -> &'static str {
    match unit {
        Unit::None => t!("units.none"),
        Unit::Percent => t!("units.percent"),
        Unit::Fraction => t!("units.fraction"),
        Unit::Bytes => t!("units.bytes"),
        Unit::BytesPerSecond => t!("units.bytes_per_second"),
        Unit::Seconds => t!("units.seconds"),
        Unit::Celsius => t!("units.celsius"),
    }
}

pub fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Value => t!("kinds.value"),
        Kind::Chart => t!("kinds.chart"),
    }
}

/// Name of a chart range, such as `1 h` or `15 min`.
pub fn range_name(seconds: u32) -> String {
    if seconds.is_multiple_of(3_600) {
        t!("age.hours", count = seconds / 3_600).to_string()
    } else {
        t!("age.minutes", count = seconds / 60).to_string()
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
            "Grafana refused the token (401 or 403)"
        );
        assert_eq!(error_message(&Error::Http(502)), "Grafana error 502");
        assert_eq!(
            error_message(&Error::Query(String::from("bad"))),
            "Query error: bad"
        );
        assert_eq!(
            invalid_message(Invalid::Url),
            "The URL must start with https:// or http://"
        );
        assert_eq!(age(42), "42 s");
        assert_eq!(age(600), "10 min");
        assert_eq!(age(7_300), "2 h");
        assert_eq!(range_name(900), "15 min");
        assert_eq!(range_name(86_400), "24 h");
        assert_eq!(unit_name(Unit::Fraction), "Percent (0 to 1)");
        assert_eq!(kind_name(Kind::Chart), "Chart");
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
    }

    #[test]
    fn masked_edits_keep_the_secret_when_typing_elsewhere_than_the_end() {
        let mut token = String::from("abcd");
        // Typed in the middle, then at the start.
        apply_masked_edit(&mut token, "**x**");
        assert_eq!(token, "abxcd");
        apply_masked_edit(&mut token, "y*****");
        assert_eq!(token, "yabxcd");
        // A pasted value replaces the whole field.
        apply_masked_edit(&mut token, "glsa_pasted");
        assert_eq!(token, "glsa_pasted");
    }
}
