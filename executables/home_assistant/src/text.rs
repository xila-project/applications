//! User-visible text built from the catalog.

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
        Error::NotFound => String::from(t!("errors.not_found")),
        Error::Http(status) => t!("errors.http", status = status).to_string(),
        Error::InvalidResponse => String::from(t!("errors.invalid_response")),
        Error::TooLarge => String::from(t!("errors.too_large")),
        Error::Storage => String::from(t!("errors.storage")),
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
            "Home Assistant refused the token (401 or 403)"
        );
        assert_eq!(error_message(&Error::NotFound), "Entity not found (404)");
        assert_eq!(error_message(&Error::Http(502)), "Home Assistant error 502");
        assert_eq!(
            error_message(&Error::MissingToken),
            "No token: add one to the token file"
        );
        assert_eq!(age(42), "42 s");
        assert_eq!(age(600), "10 min");
        assert_eq!(age(7_300), "2 h");
    }
}
