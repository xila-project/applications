//! Home Assistant service calls: the request paths and bodies.
//!
//! A service call is `POST /api/services/<domain>/<service>` with a JSON body
//! that holds the entity identifier, plus `volume_level` for
//! `media_player.volume_set`.

use crate::json::push_string;
use std::{format, string::String};

/// Path of a state answer: `GET /api/states/<entity_id>`.
pub fn state_path(entity_id: &str) -> String {
    format!("/api/states/{entity_id}")
}

/// Path of a service call: `POST /api/services/<domain>/<service>`.
pub fn service_path(domain: &str, service: &str) -> String {
    format!("/api/services/{domain}/{service}")
}

/// Body of a service call on one entity: `{"entity_id":"..."}`.
pub fn service_body(entity_id: &str) -> String {
    let mut body = String::from("{\"entity_id\":");
    push_string(&mut body, entity_id);
    body.push('}');
    body
}

/// Body of `media_player.volume_set`: the entity and its volume level, from
/// 0 to 1.
pub fn volume_body(entity_id: &str, volume_level: f64) -> String {
    let mut body = String::from("{\"entity_id\":");
    push_string(&mut body, entity_id);
    body.push_str(",\"volume_level\":");
    body.push_str(&format_volume_level(volume_level));
    body.push('}');
    body
}

/// A volume level as a JSON number; a value out of range becomes 0.
fn format_volume_level(volume_level: f64) -> String {
    if volume_level.is_finite() {
        format!("{}", volume_level.clamp(0.0, 1.0))
    } else {
        String::from("0")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_hold_the_entity_domain_and_service() {
        assert_eq!(
            state_path("light.living_room"),
            "/api/states/light.living_room"
        );
        assert_eq!(
            service_path("light", "toggle"),
            "/api/services/light/toggle"
        );
        assert_eq!(
            service_path("media_player", "volume_set"),
            "/api/services/media_player/volume_set"
        );
        assert_eq!(
            service_path("scene", "turn_on"),
            "/api/services/scene/turn_on"
        );
    }

    #[test]
    fn service_bodies_hold_the_entity_identifier() {
        assert_eq!(
            service_body("light.living_room"),
            r#"{"entity_id":"light.living_room"}"#
        );
        assert_eq!(
            service_body("scene.movie_night"),
            r#"{"entity_id":"scene.movie_night"}"#
        );
        // An identifier cannot hold a quote, but the body escapes it anyway.
        assert_eq!(service_body("a\"b"), r#"{"entity_id":"a\"b"}"#);
        assert_eq!(service_body("a\\b"), r#"{"entity_id":"a\\b"}"#);
    }

    #[test]
    fn volume_bodies_hold_the_volume_level() {
        assert_eq!(
            volume_body("media_player.television", 0.4),
            r#"{"entity_id":"media_player.television","volume_level":0.4}"#
        );
        assert_eq!(
            volume_body("media_player.radio", 1.0),
            r#"{"entity_id":"media_player.radio","volume_level":1}"#
        );
        assert_eq!(
            volume_body("media_player.radio", 0.0),
            r#"{"entity_id":"media_player.radio","volume_level":0}"#
        );
        // Out of range and non-finite levels clamp to the ends.
        assert_eq!(
            volume_body("media_player.radio", 1.5),
            r#"{"entity_id":"media_player.radio","volume_level":1}"#
        );
        assert_eq!(
            volume_body("media_player.radio", -0.2),
            r#"{"entity_id":"media_player.radio","volume_level":0}"#
        );
        assert_eq!(
            volume_body("media_player.radio", f64::NAN),
            r#"{"entity_id":"media_player.radio","volume_level":0}"#
        );
        // The bodies parse back as JSON.
        for body in [service_body("light.a"), volume_body("media_player.b", 0.25)] {
            let mut reader =
                crate::json::Reader::new(crate::json::SliceSource::new(body.as_bytes()));
            reader
                .object(|reader, key| {
                    assert!(matches!(key, "entity_id" | "volume_level"));
                    reader.skip()
                })
                .unwrap();
            reader.finish().unwrap();
        }
    }
}
