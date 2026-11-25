//! The configuration file: the Home Assistant URL, the refresh period and
//! the ordered list of entity identifiers to show. The access token lives in
//! its own file next to it, see [`crate::storage`].

use crate::error::{Error, Result};
use miniserde::{Deserialize, Serialize, json};
use std::{string::String, vec::Vec};

/// Refresh period used when the configuration does not set one.
pub const DEFAULT_REFRESH_SECONDS: u32 = 30;
/// Shortest refresh period accepted.
pub const MINIMUM_REFRESH_SECONDS: u32 = 10;
/// Most entities shown.
pub const MAXIMUM_ENTITIES: usize = 24;
/// Longest entity identifier kept, such as `light.living_room`.
pub const MAXIMUM_ENTITY_ID_LENGTH: usize = 64;

/// What the user can get wrong in the configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    Url,
    Refresh,
    Entity,
    TooManyEntities,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Configuration {
    /// Home Assistant base URL, such as `http://homeassistant.local:8123`.
    pub url: String,
    pub refresh_seconds: u32,
    /// Entity identifiers to show, in order.
    pub entities: Vec<String>,
}

/// The file layout: only the refresh period is optional, so a file written
/// by hand stays short.
#[derive(Serialize, Deserialize)]
struct File {
    url: String,
    refresh_seconds: Option<u32>,
    entities: Vec<String>,
}

impl Configuration {
    pub fn parse(text: &str) -> Result<Self> {
        let file: File = json::from_str(text).map_err(|_| Error::Configuration)?;
        Ok(Self {
            url: file.url,
            refresh_seconds: file.refresh_seconds.unwrap_or(DEFAULT_REFRESH_SECONDS),
            entities: file.entities,
        })
    }

    pub fn to_json(&self) -> String {
        json::to_string(&File {
            url: self.url.clone(),
            refresh_seconds: Some(self.refresh_seconds),
            entities: self.entities.clone(),
        })
    }

    pub fn refresh_seconds(&self) -> u32 {
        self.refresh_seconds.max(MINIMUM_REFRESH_SECONDS)
    }

    /// Checks the connection and every entity identifier.
    pub fn validate(&self) -> core::result::Result<(), Invalid> {
        validate_url(&self.url)?;
        if self.refresh_seconds < MINIMUM_REFRESH_SECONDS {
            return Err(Invalid::Refresh);
        }
        if self.entities.len() > MAXIMUM_ENTITIES {
            return Err(Invalid::TooManyEntities);
        }
        if self
            .entities
            .iter()
            .any(|entity_id| !is_valid_entity_id(entity_id))
        {
            return Err(Invalid::Entity);
        }
        Ok(())
    }
}

/// Accepts `http://` and `https://` URLs with a host and no spaces.
pub fn validate_url(url: &str) -> core::result::Result<(), Invalid> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or(Invalid::Url)?;
    let host = rest.split('/').next().unwrap_or("");
    if host.is_empty()
        || url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(Invalid::Url);
    }
    Ok(())
}

/// An entity identifier holds a domain and an object identifier, both
/// lowercase, such as `light.living_room`.
pub fn is_valid_entity_id(entity_id: &str) -> bool {
    if entity_id.is_empty() || entity_id.len() > MAXIMUM_ENTITY_ID_LENGTH {
        return false;
    }
    let Some((domain, object)) = entity_id.split_once('.') else {
        return false;
    };
    let valid_domain = domain
        .bytes()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && domain
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    let valid_object = !object.is_empty()
        && object
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    valid_domain && valid_object
}

/// The domain part of an entity identifier, such as `light`.
pub fn domain_of(entity_id: &str) -> &str {
    entity_id
        .split_once('.')
        .map_or(entity_id, |(domain, _)| domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Configuration {
        Configuration {
            url: String::from("http://homeassistant.local:8123"),
            refresh_seconds: 30,
            entities: std::vec![
                String::from("light.living_room"),
                String::from("sensor.outdoor_temperature"),
                String::from("media_player.television"),
            ],
        }
    }

    #[test]
    fn round_trips_through_the_file_format() {
        let configuration = example();
        assert_eq!(
            Configuration::parse(&configuration.to_json()).unwrap(),
            configuration
        );
    }

    #[test]
    fn hand_written_files_only_need_the_url_and_the_entities() {
        let configuration = Configuration::parse(
            r#"{"url": "http://192.168.1.20:8123", "entities": ["switch.pump"]}"#,
        )
        .unwrap();
        assert_eq!(configuration.refresh_seconds, DEFAULT_REFRESH_SECONDS);
        assert_eq!(configuration.refresh_seconds(), DEFAULT_REFRESH_SECONDS);
        assert_eq!(configuration.entities, ["switch.pump"]);
        assert_eq!(configuration.validate(), Ok(()));
        assert_eq!(Configuration::parse("{}"), Err(Error::Configuration));
        assert_eq!(
            Configuration::parse(r#"{"url": "http://x", "entities": "light.a"}"#),
            Err(Error::Configuration)
        );
        assert_eq!(
            Configuration::parse(r#"{"url": "http://x", "entities": [1]}"#),
            Err(Error::Configuration)
        );
    }

    #[test]
    fn validation_catches_user_mistakes() {
        let mut configuration = example();
        assert_eq!(configuration.validate(), Ok(()));
        configuration.url = String::from("homeassistant.local:8123");
        assert_eq!(configuration.validate(), Err(Invalid::Url));
        configuration.url = String::from("ftp://homeassistant.local");
        assert_eq!(configuration.validate(), Err(Invalid::Url));
        configuration.url = String::from("https://");
        assert_eq!(configuration.validate(), Err(Invalid::Url));
        configuration.url = String::from("http://homeassistant.local:8123/api");
        assert_eq!(configuration.validate(), Ok(()));
        configuration.refresh_seconds = 2;
        assert_eq!(configuration.validate(), Err(Invalid::Refresh));
        configuration.refresh_seconds = 30;
        configuration.entities.push(String::from("Light.Ceiling"));
        assert_eq!(configuration.validate(), Err(Invalid::Entity));
        configuration.entities.pop();
        configuration.entities.push(String::from("light"));
        assert_eq!(configuration.validate(), Err(Invalid::Entity));
        configuration.entities.pop();
        configuration.entities.push(String::from("light.a/b"));
        assert_eq!(configuration.validate(), Err(Invalid::Entity));
        configuration.entities.pop();
        configuration.entities = (0..25)
            .map(|index| std::format!("sensor.s{index}"))
            .collect();
        assert_eq!(configuration.validate(), Err(Invalid::TooManyEntities));
    }

    #[test]
    fn entity_identifiers_and_domains() {
        assert!(is_valid_entity_id("light.living_room"));
        assert!(is_valid_entity_id("sensor.2nd_floor_temperature"));
        assert!(is_valid_entity_id("input_boolean.guest_mode"));
        assert!(!is_valid_entity_id(""));
        assert!(!is_valid_entity_id("light"));
        assert!(!is_valid_entity_id(".living_room"));
        assert!(!is_valid_entity_id("light."));
        assert!(!is_valid_entity_id("Light.living_room"));
        assert!(!is_valid_entity_id("light.living room"));
        assert!(!is_valid_entity_id("light.living-room"));
        assert!(!is_valid_entity_id(&"a.".repeat(40)));
        assert_eq!(domain_of("light.living_room"), "light");
        assert_eq!(domain_of("no_dot"), "no_dot");
    }
}
