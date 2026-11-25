//! The entity picker of the settings screen: the areas of the Home Assistant
//! installation and the entities of an area, read through `POST /api/template`
//! with small templates, so that the application never downloads all states
//! (large installations return megabytes from `/api/states`).
//!
//! A template renders one line per entry, `identifier|name`: the separator
//! cannot appear in an entity identifier or an area identifier, so the output
//! parses safely. The answer is plain text and bounded.

use crate::configuration::is_valid_entity_id;
use crate::error::{Error, Result};
use crate::state::Domain;
use std::{format, string::String, vec::Vec};

/// Separates an identifier from its name on one line. Entity and area
/// identifiers hold only lowercase letters, digits and underscores.
pub const SEPARATOR: char = '|';
/// Most entries read from one template answer.
pub const MAXIMUM_ENTRIES: usize = 256;
/// The loop index at which a bounded template marks a truncated list.
pub const MAXIMUM_ENTRIES_PLUS_ONE: usize = MAXIMUM_ENTRIES + 1;
/// Largest template answer read.
pub const MAXIMUM_TEMPLATE_ANSWER_LENGTH: usize = 64 * 1024;
/// Longest line kept, so a runaway answer does not fill memory.
const MAXIMUM_LINE_LENGTH: usize = 256;

/// One entry of a picker list: an identifier and its friendly name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub identifier: String,
    pub name: String,
}

/// The domains the picker offers, as a Jinja list for the templates.
const SUPPORTED_DOMAINS: &str =
    "['light','switch','input_boolean','scene','script','sensor','binary_sensor','media_player']";

/// Sanitizes a name for one output line: a missing name falls back to the
/// identifier, and the separator and line ends become spaces, so a name can
/// never add rows or split a line.
const SANITIZE_NAME: &str = " | replace('|', ' ') | replace('\\n', ' ') | replace('\\r', ' ')";

/// Template listing the areas, one per line: `area_id|area name`.
pub fn areas_template() -> String {
    format!(
        "{{% for area in areas() %}}{{{{ area }}}}{SEPARATOR}{{{{ (area_name(area) or area){SANITIZE_NAME} }}}}\n{{% endfor %}}"
    )
}

/// Template listing the supported entities of one area, one per line:
/// `entity_id|friendly name`. The area identifier is validated, so it sits
/// safely inside the quotes.
pub fn area_entities_template(area_id: &str) -> Result<String> {
    if area_id.is_empty()
        || !area_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(Error::InvalidResponse);
    }
    Ok(format!(
        "{{% for entity in area_entities('{area_id}') if entity.split('.')[0] in {SUPPORTED_DOMAINS} %}}{{{{ entity }}}}{SEPARATOR}{{{{ (state_attr(entity, 'friendly_name') or entity){SANITIZE_NAME} }}}}\n{{% endfor %}}"
    ))
}

/// Template listing the supported entities that no area holds, one per
/// line. The loop emits a row only while the index is at most
/// [`MAXIMUM_ENTRIES`], and marks a truncated list at the next index, so a
/// huge installation cannot flood the answer with rows the application
/// would refuse anyway.
pub fn no_area_entities_template() -> String {
    format!(
        "{{% set in_areas = areas() | map('area_entities') | sum(start=[]) %}}{{% for state in states if state.entity_id.split('.')[0] in {SUPPORTED_DOMAINS} and state.entity_id not in in_areas %}}{{% if loop.index <= {MAXIMUM_ENTRIES} %}}{{{{ state.entity_id }}}}{SEPARATOR}{{{{ (state_attr(state.entity_id, 'friendly_name') or state.entity_id){SANITIZE_NAME} }}}}\n{{% endif %}}{{% if loop.index == {MAXIMUM_ENTRIES_PLUS_ONE} %}}@@truncated@@\n{{% endif %}}{{% endfor %}}"
    )
}

/// Body of `POST /api/template` for `template`.
pub fn template_body(template: &str) -> String {
    let mut body = String::from("{\"template\":");
    crate::json::push_string(&mut body, template);
    body.push('}');
    body
}

/// One picker answer: the entries and whether the server truncated the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

/// Marker line a bounded template emits when it stopped early.
const TRUNCATED_MARKER: &str = "@@truncated@@";

/// Parses a picker answer: one `identifier|name` per line. Lines that do not
/// hold a supported identifier are skipped, duplicates collapse, and the
/// list stops at [`MAXIMUM_ENTRIES`] with `truncated` set when the answer
/// held more rows. `entities_only` decides whether the
/// identifier is kept: areas hold any identifier, entities only the domains
/// the application shows. A line longer than the limit is cut at a character
/// boundary, so it can never panic on a multi-byte character.
pub fn parse_entries(text: &str, entities_only: bool) -> Answer {
    let mut entries: Vec<Entry> = Vec::new();
    let mut truncated = false;
    for line in text.lines() {
        if line == TRUNCATED_MARKER {
            truncated = true;
            continue;
        }
        if entries.len() >= MAXIMUM_ENTRIES {
            // The answer holds more rows than the limit: the list is cut.
            truncated = true;
            break;
        }
        let line = if line.len() > MAXIMUM_LINE_LENGTH {
            let mut end = MAXIMUM_LINE_LENGTH;
            while !line.is_char_boundary(end) {
                end -= 1;
            }
            &line[..end]
        } else {
            line
        };
        let Some((identifier, name)) = line.split_once(SEPARATOR) else {
            continue;
        };
        if entities_only {
            if !is_valid_entity_id(identifier) || !Domain::parse(identifier).is_supported() {
                continue;
            }
        } else if identifier.is_empty()
            || !identifier
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            continue;
        }
        if entries.iter().any(|entry| entry.identifier == identifier) {
            continue;
        }
        entries.push(Entry {
            identifier: String::from(identifier),
            name: String::from(name.trim()),
        });
    }
    Answer { entries, truncated }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_hold_one_line_per_entry() {
        assert_eq!(
            areas_template(),
            "{% for area in areas() %}{{ area }}|{{ (area_name(area) or area) | replace('|', ' ') | replace('\\n', ' ') | replace('\\r', ' ') }}\n{% endfor %}"
        );
        assert_eq!(
            area_entities_template("living_room").unwrap(),
            "{% for entity in area_entities('living_room') if entity.split('.')[0] in ['light','switch','input_boolean','scene','script','sensor','binary_sensor','media_player'] %}{{ entity }}|{{ (state_attr(entity, 'friendly_name') or entity) | replace('|', ' ') | replace('\\n', ' ') | replace('\\r', ' ') }}\n{% endfor %}"
        );
        assert!(area_entities_template("bad'id").is_err());
        assert!(area_entities_template("").is_err());
        assert!(area_entities_template("Living Room").is_err());
        let no_area = no_area_entities_template();
        assert!(no_area.contains("not in in_areas"));
        // The "no area" loop emits a row only up to the limit, and marks a
        // truncated list at the next index.
        assert!(no_area.contains("{% if loop.index <= 256 %}"));
        assert!(no_area.contains("{% if loop.index == 257 %}@@truncated@@\n{% endif %}"));
        assert!(!no_area.contains("{% if loop.index == 256 %}@@truncated@@"));
        assert_eq!(
            template_body("{{ 1 + 1 }}"),
            "{\"template\":\"{{ 1 + 1 }}\"}"
        );
        assert_eq!(
            template_body("a\"b\\c\n"),
            "{\"template\":\"a\\\"b\\\\c\\n\"}"
        );
    }

    #[test]
    fn parses_area_answers() {
        let answer = parse_entries("living_room|Living room\nkitchen|Kitchen\n", false);
        assert_eq!(
            answer.entries,
            std::vec![
                Entry {
                    identifier: String::from("living_room"),
                    name: String::from("Living room"),
                },
                Entry {
                    identifier: String::from("kitchen"),
                    name: String::from("Kitchen"),
                },
            ]
        );
        assert!(!answer.truncated);
        // A name may hold the separator: only the first one splits.
        let answer = parse_entries("kitchen|Kitchen | open\n", false);
        assert_eq!(answer.entries[0].name, "Kitchen | open");
        // Bad lines are skipped, duplicates collapse.
        let answer = parse_entries("bad id|X\nkitchen|Kitchen\nkitchen|Other\n\n", false);
        assert_eq!(answer.entries.len(), 1);
        assert_eq!(answer.entries[0].identifier, "kitchen");
        // The truncation marker is reported.
        let answer = parse_entries("kitchen|Kitchen\n@@truncated@@\n", false);
        assert_eq!(answer.entries.len(), 1);
        assert!(answer.truncated);
    }

    #[test]
    fn parses_entity_answers_with_supported_domains_only() {
        let text = "light.living_room|Living room\nsensor.temp|Temp\nweather.home|Home\nLight.Bad|X\nlight.living_room|Dup\n";
        let answer = parse_entries(text, true);
        assert_eq!(answer.entries.len(), 2);
        assert_eq!(answer.entries[0].identifier, "light.living_room");
        assert_eq!(answer.entries[0].name, "Living room");
        assert_eq!(answer.entries[1].identifier, "sensor.temp");
        // An empty name stays empty.
        let answer = parse_entries("switch.pump|\n", true);
        assert_eq!(answer.entries.len(), 1);
        assert_eq!(answer.entries[0].name, "");
        // The list is bounded, and a cut list is reported as truncated.
        let many = (0..500)
            .map(|index| format!("sensor.s{index}|S{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let answer = parse_entries(&many, true);
        assert_eq!(answer.entries.len(), MAXIMUM_ENTRIES);
        assert!(answer.truncated);
        // An answer that stops exactly at the limit is not truncated.
        let exact = (0..MAXIMUM_ENTRIES)
            .map(|index| format!("sensor.s{index}|S{index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let answer = parse_entries(&exact, true);
        assert_eq!(answer.entries.len(), MAXIMUM_ENTRIES);
        assert!(!answer.truncated);
        // A long line is cut at a character boundary, not inside one.
        let mut long = String::from("sensor.long|");
        long.push_str(&"é".repeat(300));
        let answer = parse_entries(&long, true);
        assert_eq!(answer.entries.len(), 1);
        assert!(answer.entries[0].name.len() <= MAXIMUM_LINE_LENGTH);
        assert!(answer.entries[0].name.chars().all(|c| c == 'é'));
    }
}
