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
        Error::Forbidden => String::from(t!("errors.forbidden")),
        Error::NotFound => String::from(t!("errors.not_found")),
        Error::AdminNeeded => String::from(t!("errors.admin_needed")),
        Error::ImportMissing => String::from(t!("errors.import_missing")),
        Error::ImportEmpty => String::from(t!("errors.import_empty")),
        Error::ImportInvalid => String::from(t!("errors.import_invalid")),
        Error::ImportTooLong => String::from(t!("errors.import_too_long")),
        Error::Http(status) => t!("errors.http", status = status).to_string(),
        Error::InvalidResponse => String::from(t!("errors.invalid_response")),
        Error::TooLarge => String::from(t!("errors.too_large")),
        Error::Storage => String::from(t!("errors.storage")),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_come_from_the_catalog() {
        assert_eq!(
            error_message(&Error::Unauthorized),
            "Home Assistant refused the token (401)"
        );
        assert_eq!(
            error_message(&Error::Forbidden),
            "Home Assistant refused the request (403)"
        );
        assert_eq!(error_message(&Error::NotFound), "Not found (404)");
        assert_eq!(error_message(&Error::Http(502)), "Home Assistant error 502");
        assert_eq!(
            error_message(&Error::MissingToken),
            "No token: add one in the settings or import it from a file"
        );
        assert_eq!(
            error_message(&Error::AdminNeeded),
            "The area picker needs an administrator account"
        );
        assert_eq!(
            error_message(&Error::ImportMissing),
            "The file does not exist"
        );
        assert_eq!(
            error_message(&Error::ImportEmpty),
            "The file holds no token"
        );
        assert_eq!(
            error_message(&Error::ImportInvalid),
            "The token holds characters a header cannot carry"
        );
        assert_eq!(
            error_message(&Error::ImportTooLong),
            "The token is too long (at most 512 bytes)"
        );
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
}
