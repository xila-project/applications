//! Entity states of the Home Assistant REST API, the domains that decide
//! the controls, and the streaming parser of a state answer.
//!
//! `GET /api/states/<entity_id>` answers one object: the entity identifier,
//! its state, and its attributes. Only the attributes the application shows
//! are kept; the rest is skipped while streaming.

use crate::configuration::domain_of;
use crate::error::{Error, Result};
use crate::json::{Reader, Source};
use std::{format, string::String};

/// Longest state string kept, such as `unavailable`.
pub const MAXIMUM_STATE_LENGTH: usize = 32;
/// Longest friendly name kept.
pub const MAXIMUM_NAME_LENGTH: usize = 64;
/// Longest unit of measurement kept.
pub const MAXIMUM_UNIT_LENGTH: usize = 16;
/// Longest media title or artist kept.
pub const MAXIMUM_MEDIA_LENGTH: usize = 128;

/// The domain of an entity, from the part before the dot of its identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Light,
    Switch,
    InputBoolean,
    Scene,
    Script,
    Sensor,
    BinarySensor,
    MediaPlayer,
    /// A domain the application shows as a plain value.
    Other,
}

impl Domain {
    pub fn parse(entity_id: &str) -> Self {
        match domain_of(entity_id) {
            "light" => Self::Light,
            "switch" => Self::Switch,
            "input_boolean" => Self::InputBoolean,
            "scene" => Self::Scene,
            "script" => Self::Script,
            "sensor" => Self::Sensor,
            "binary_sensor" => Self::BinarySensor,
            "media_player" => Self::MediaPlayer,
            _ => Self::Other,
        }
    }

    /// Whether the domain is a value only, without controls.
    pub fn is_value_only(&self) -> bool {
        matches!(self, Self::Sensor | Self::BinarySensor | Self::Other)
    }
}

/// A control shown on an entity card. Each control calls one service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// `toggle`, for lights, switches and input booleans.
    Toggle,
    /// `turn_on`, for scenes and scripts.
    TurnOn,
    PlayPause,
    NextTrack,
    PreviousTrack,
    VolumeUp,
    VolumeDown,
}

impl Control {
    /// The Home Assistant service the control calls, in the domain of the
    /// entity.
    pub fn service(&self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::TurnOn => "turn_on",
            Self::PlayPause => "media_play_pause",
            Self::NextTrack => "media_next_track",
            Self::PreviousTrack => "media_previous_track",
            Self::VolumeUp => "volume_up",
            Self::VolumeDown => "volume_down",
        }
    }
}

/// The controls of a domain, in the order they show on the card.
pub fn controls(domain: Domain) -> &'static [Control] {
    match domain {
        Domain::Light | Domain::Switch | Domain::InputBoolean => &[Control::Toggle],
        Domain::Scene | Domain::Script => &[Control::TurnOn],
        Domain::MediaPlayer => &[
            Control::PreviousTrack,
            Control::PlayPause,
            Control::NextTrack,
            Control::VolumeDown,
            Control::VolumeUp,
        ],
        Domain::Sensor | Domain::BinarySensor | Domain::Other => &[],
    }
}

/// One entity state, with only the attributes the application shows.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EntityState {
    pub entity_id: String,
    pub state: String,
    /// `attributes.friendly_name`.
    pub friendly_name: Option<String>,
    /// `attributes.unit_of_measurement`.
    pub unit_of_measurement: Option<String>,
    /// `attributes.media_title`.
    pub media_title: Option<String>,
    /// `attributes.media_artist`.
    pub media_artist: Option<String>,
    /// `attributes.volume_level`, from 0 to 1.
    pub volume_level: Option<f64>,
}

impl EntityState {
    /// The name shown on the card: the friendly name, else the identifier.
    pub fn display_name(&self) -> &str {
        self.friendly_name.as_deref().unwrap_or(&self.entity_id)
    }

    /// The value line: the state, with its unit when it has one.
    pub fn value_text(&self) -> String {
        match &self.unit_of_measurement {
            Some(unit) if !unit.is_empty() => format!("{} {}", self.state, unit),
            _ => self.state.clone(),
        }
    }

    /// The media line: the artist and the title, when the player has them.
    pub fn media_text(&self) -> Option<String> {
        match (&self.media_artist, &self.media_title) {
            (Some(artist), Some(title)) if !artist.is_empty() && !title.is_empty() => {
                Some(format!("{artist} - {title}"))
            }
            (None, Some(title)) => Some(title.clone()),
            (Some(artist), _) if !artist.is_empty() => Some(artist.clone()),
            _ => None,
        }
    }

    /// The volume as a percentage, such as `40 %`, when the player reports it.
    pub fn volume_text(&self) -> Option<String> {
        self.volume_level
            .filter(|level| level.is_finite())
            .map(|level| format!("{:.0} %", level.clamp(0.0, 1.0) * 100.0))
    }
}

/// Parses a state answer. The reader must sit at the start of the object.
pub fn parse<S: Source>(reader: &mut Reader<S>) -> Result<EntityState> {
    let mut entity_id = None;
    let mut state = None;
    let mut attributes = EntityState::default();
    reader.object(|reader, key| {
        match key {
            "entity_id" => {
                entity_id = Some(reader.string(crate::configuration::MAXIMUM_ENTITY_ID_LENGTH)?)
            }
            "state" => state = Some(reader.string(MAXIMUM_STATE_LENGTH)?),
            "attributes" => parse_attributes(reader, &mut attributes)?,
            _ => reader.skip()?,
        }
        Ok(())
    })?;
    reader.finish()?;
    let entity_id = entity_id.ok_or(Error::InvalidResponse)?;
    let state = state.ok_or(Error::InvalidResponse)?;
    Ok(EntityState {
        entity_id,
        state,
        friendly_name: attributes.friendly_name,
        unit_of_measurement: attributes.unit_of_measurement,
        media_title: attributes.media_title,
        media_artist: attributes.media_artist,
        volume_level: attributes.volume_level,
    })
}

/// Parses the `attributes` object into `state`; only the shown attributes
/// are kept.
fn parse_attributes<S: Source>(reader: &mut Reader<S>, state: &mut EntityState) -> Result<()> {
    reader.object(|reader, key| {
        match key {
            "friendly_name" => state.friendly_name = reader.optional_string(MAXIMUM_NAME_LENGTH)?,
            "unit_of_measurement" => {
                state.unit_of_measurement = reader.optional_string(MAXIMUM_UNIT_LENGTH)?
            }
            "media_title" => state.media_title = reader.optional_string(MAXIMUM_MEDIA_LENGTH)?,
            "media_artist" => state.media_artist = reader.optional_string(MAXIMUM_MEDIA_LENGTH)?,
            "volume_level" => state.volume_level = reader.optional_number()?,
            _ => reader.skip()?,
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::SliceSource;
    use std::string::String;

    fn parse_str(text: &str) -> Result<EntityState> {
        parse(&mut Reader::new(SliceSource::new(text.as_bytes())))
    }

    #[test]
    fn domains_map_to_their_controls() {
        assert_eq!(Domain::parse("light.living_room"), Domain::Light);
        assert_eq!(Domain::parse("switch.pump"), Domain::Switch);
        assert_eq!(
            Domain::parse("input_boolean.guest_mode"),
            Domain::InputBoolean
        );
        assert_eq!(Domain::parse("scene.movie_night"), Domain::Scene);
        assert_eq!(Domain::parse("script.morning"), Domain::Script);
        assert_eq!(Domain::parse("sensor.temperature"), Domain::Sensor);
        assert_eq!(Domain::parse("binary_sensor.door"), Domain::BinarySensor);
        assert_eq!(
            Domain::parse("media_player.television"),
            Domain::MediaPlayer
        );
        assert_eq!(Domain::parse("weather.home"), Domain::Other);
        assert_eq!(controls(Domain::Light), &[Control::Toggle]);
        assert_eq!(controls(Domain::Switch), &[Control::Toggle]);
        assert_eq!(controls(Domain::InputBoolean), &[Control::Toggle]);
        assert_eq!(controls(Domain::Scene), &[Control::TurnOn]);
        assert_eq!(controls(Domain::Script), &[Control::TurnOn]);
        assert_eq!(
            controls(Domain::MediaPlayer),
            &[
                Control::PreviousTrack,
                Control::PlayPause,
                Control::NextTrack,
                Control::VolumeDown,
                Control::VolumeUp
            ]
        );
        assert!(controls(Domain::Sensor).is_empty());
        assert!(controls(Domain::BinarySensor).is_empty());
        assert!(controls(Domain::Other).is_empty());
        assert!(Domain::Sensor.is_value_only());
        assert!(!Domain::Light.is_value_only());
        assert_eq!(Control::Toggle.service(), "toggle");
        assert_eq!(Control::TurnOn.service(), "turn_on");
        assert_eq!(Control::PlayPause.service(), "media_play_pause");
        assert_eq!(Control::NextTrack.service(), "media_next_track");
        assert_eq!(Control::PreviousTrack.service(), "media_previous_track");
        assert_eq!(Control::VolumeUp.service(), "volume_up");
        assert_eq!(Control::VolumeDown.service(), "volume_down");
    }

    #[test]
    fn parses_a_light_state() {
        let state = parse_str(include_str!("../tests/fixtures/light.json")).unwrap();
        assert_eq!(state.entity_id, "light.living_room");
        assert_eq!(state.state, "on");
        assert_eq!(state.display_name(), "Living room");
        assert_eq!(state.value_text(), "on");
        assert_eq!(state.media_text(), None);
        assert_eq!(state.volume_text(), None);
    }

    #[test]
    fn parses_a_sensor_state_with_its_unit() {
        let state = parse_str(include_str!("../tests/fixtures/sensor.json")).unwrap();
        assert_eq!(state.entity_id, "sensor.outdoor_temperature");
        assert_eq!(state.state, "21.5");
        assert_eq!(state.unit_of_measurement.as_deref(), Some("°C"));
        assert_eq!(state.value_text(), "21.5 °C");
        assert_eq!(state.display_name(), "Outdoor temperature");
    }

    #[test]
    fn parses_a_binary_sensor_state() {
        let state = parse_str(include_str!("../tests/fixtures/binary_sensor.json")).unwrap();
        assert_eq!(state.entity_id, "binary_sensor.front_door");
        assert_eq!(state.state, "off");
        assert_eq!(state.value_text(), "off");
    }

    #[test]
    fn parses_a_media_player_state() {
        let state = parse_str(include_str!("../tests/fixtures/media_player.json")).unwrap();
        assert_eq!(state.entity_id, "media_player.television");
        assert_eq!(state.state, "playing");
        assert_eq!(state.media_title.as_deref(), Some("The News"));
        assert_eq!(state.media_artist.as_deref(), Some("Channel One"));
        assert_eq!(
            state.media_text().as_deref(),
            Some("Channel One - The News")
        );
        assert_eq!(state.volume_level, Some(0.4));
        assert_eq!(state.volume_text().as_deref(), Some("40 %"));
    }

    #[test]
    fn parses_switch_input_boolean_scene_and_script_states() {
        let switch = parse_str(include_str!("../tests/fixtures/switch.json")).unwrap();
        assert_eq!(switch.entity_id, "switch.pump");
        assert_eq!(switch.state, "off");
        let input_boolean =
            parse_str(include_str!("../tests/fixtures/input_boolean.json")).unwrap();
        assert_eq!(input_boolean.entity_id, "input_boolean.guest_mode");
        assert_eq!(input_boolean.state, "on");
        let scene = parse_str(include_str!("../tests/fixtures/scene.json")).unwrap();
        assert_eq!(scene.entity_id, "scene.movie_night");
        let script = parse_str(include_str!("../tests/fixtures/script.json")).unwrap();
        assert_eq!(script.entity_id, "script.morning");
        assert_eq!(script.state, "on");
    }

    #[test]
    fn skips_attributes_it_does_not_show() {
        let state = parse_str(
            r#"{"entity_id": "light.porch", "state": "off", "attributes": {
                "friendly_name": "Porch", "brightness": 255, "rgb_color": [255, 244, 229],
                "supported_color_modes": ["onoff"], "effect_list": null, "effect": null,
                "restored": true, "supported_features": 44, "icon": "mdi:lightbulb"
            }, "last_changed": "2026-01-01T00:00:00.000000+00:00",
            "last_updated": "2026-01-01T00:00:00.000000+00:00",
            "context": {"id": "01J", "parent_id": null, "user_id": null}}"#,
        )
        .unwrap();
        assert_eq!(state.state, "off");
        assert_eq!(state.display_name(), "Porch");
        assert_eq!(state.unit_of_measurement, None);
    }

    #[test]
    fn media_text_uses_what_is_there() {
        let mut state = EntityState {
            entity_id: String::from("media_player.radio"),
            state: String::from("paused"),
            ..EntityState::default()
        };
        assert_eq!(state.media_text(), None);
        state.media_title = Some(String::from("Song"));
        assert_eq!(state.media_text().as_deref(), Some("Song"));
        state.media_artist = Some(String::from("Artist"));
        assert_eq!(state.media_text().as_deref(), Some("Artist - Song"));
        state.volume_level = Some(1.0);
        assert_eq!(state.volume_text().as_deref(), Some("100 %"));
        state.volume_level = Some(7.5);
        assert_eq!(state.volume_text().as_deref(), Some("100 %"));
    }

    #[test]
    fn refuses_answers_that_are_not_a_state() {
        assert_eq!(parse_str("{}"), Err(Error::InvalidResponse));
        assert_eq!(
            parse_str(r#"{"entity_id": "light.a"}"#),
            Err(Error::InvalidResponse)
        );
        assert_eq!(
            parse_str(r#"{"entity_id": "light.a", "state": 3}"#),
            Err(Error::InvalidResponse)
        );
        assert_eq!(parse_str("[1, 2]"), Err(Error::InvalidResponse));
        assert_eq!(
            parse_str(r#"{"entity_id": "light.a", "state": "on", "attributes": 5}"#),
            Err(Error::InvalidResponse)
        );
        assert_eq!(
            parse_str(r#"{"entity_id": "light.a", "state": "on"} trailing"#),
            Err(Error::InvalidResponse)
        );
        let long_name = std::format!(
            r#"{{"entity_id": "light.a", "state": "on", "attributes": {{"friendly_name": "{}"}}}}"#,
            "x".repeat(MAXIMUM_NAME_LENGTH + 1)
        );
        assert_eq!(parse_str(&long_name), Err(Error::TooLarge));
    }

    #[test]
    fn reads_states_in_fragments() {
        // The same answer, byte by byte through the streaming reader.
        let text = include_str!("../tests/fixtures/sensor.json");
        let response =
            std::format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{text}");
        let (status, body) =
            crate::net::read_response(crate::net::tests::canned(&response, 1)).unwrap();
        assert_eq!(status, 200);
        let state = parse(&mut Reader::new(body)).unwrap();
        assert_eq!(state.entity_id, "sensor.outdoor_temperature");
        assert_eq!(state.value_text(), "21.5 °C");
    }
}
