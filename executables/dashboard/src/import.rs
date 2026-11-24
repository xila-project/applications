//! One-shot conversion of a Grafana dashboard (`GET /api/dashboards/uid/<uid>`)
//! into panels.
//!
//! - stat, gauge and bargauge panels become value panels, timeseries panels
//!   become charts over the dashboard's time range (at most a day); rows are
//!   walked, collapsed ones included, and every other type is skipped;
//! - each panel keeps its title, first visible target's expression, unit,
//!   decimals and first two threshold steps (as warning and critical);
//! - template variables take their current value (`All` becomes the
//!   variable's "all" value or `.*`, several values `a|b`), and
//!   `$__rate_interval`, `$__interval` and `$__range` become `5m`;
//! - data source references are ignored: the configured one is used.
//!
//! The response is streamed: only the kept panels are held in memory.

use crate::configuration::{
    DEFAULT_RANGE_SECONDS, Imported, Kind, MAXIMUM_EXPRESSION_LENGTH, MAXIMUM_PANELS,
    MAXIMUM_RANGE_SECONDS, MAXIMUM_TITLE_LENGTH, Panel,
};
use crate::error::{Error, Result};
use crate::format::{MAXIMUM_DECIMALS, Unit};
use crate::json::{Kind as JsonKind, Reader, Source};
use std::{format, string::String, vec::Vec};

/// Fixed window used for Grafana's interval macros.
const INTERVAL: &str = "5m";
const MAXIMUM_VARIABLE_LENGTH: usize = 256;
/// Most variables and, for one variable, selected values kept. A variable
/// beyond these limits is not substituted, so its panels show a query error
/// instead of values for a part of the selection.
const MAXIMUM_VARIABLES: usize = 64;
const MAXIMUM_VARIABLE_VALUES: usize = 16;

/// A panel as read, before its type is known.
#[derive(Default)]
struct RawPanel {
    kind: String,
    title: String,
    id: Option<f64>,
    expression: Option<String>,
    unit: Option<String>,
    decimals: Option<f64>,
    thresholds: Vec<f64>,
}

/// Panels read so far, with the number skipped.
#[derive(Default)]
struct Collected {
    panels: Vec<(Panel, Kind)>,
    skipped: u32,
}

impl Collected {
    fn add(&mut self, raw: RawPanel) {
        let kind = match raw.kind.as_str() {
            "stat" | "gauge" | "bargauge" => Kind::Value,
            "timeseries" => Kind::Chart,
            "row" => return,
            _ => {
                self.skipped += 1;
                return;
            }
        };
        let Some(expression) = raw.expression else {
            self.skipped += 1;
            return;
        };
        if self.panels.len() == MAXIMUM_PANELS {
            self.skipped += 1;
            return;
        }
        let mut title = raw.title;
        if title.len() > MAXIMUM_TITLE_LENGTH {
            let mut end = MAXIMUM_TITLE_LENGTH;
            while !title.is_char_boundary(end) {
                end -= 1;
            }
            title.truncate(end);
        }
        let panel = Panel {
            key: raw
                .id
                .filter(|id| id.is_finite() && *id >= 0.0 && *id < 1e15)
                .map(|id| format!("{}", id as u64)),
            title,
            expression,
            unit: Some(Unit::from_grafana(raw.unit.as_deref().unwrap_or(""))),
            decimals: raw
                .decimals
                .filter(|decimals| decimals.is_finite() && *decimals >= 0.0)
                .map(|decimals| (decimals as u8).min(MAXIMUM_DECIMALS)),
            warning: raw.thresholds.first().copied(),
            critical: raw.thresholds.get(1).copied(),
            kind: Some(kind),
            range_seconds: None,
        };
        self.panels.push((panel, kind));
    }
}

fn parse_targets<S: Source>(json: &mut Reader<S>, raw: &mut RawPanel) -> Result<()> {
    json.array(|json| {
        if json.kind()? != JsonKind::Object {
            return json.skip();
        }
        let mut expression = None;
        let mut hidden = false;
        json.object(|json, key| {
            match key {
                "expr" if json.kind()? == JsonKind::String => {
                    match json.string(MAXIMUM_EXPRESSION_LENGTH) {
                        Ok(text) => expression = Some(text),
                        // Too long to keep: the panel is skipped.
                        Err(Error::TooLarge) => (),
                        Err(error) => return Err(error),
                    }
                }
                "hide" => hidden = json.optional_boolean()?.unwrap_or(false),
                _ => json.skip()?,
            }
            Ok(())
        })?;
        if raw.expression.is_none() && !hidden {
            raw.expression = expression.filter(|text| !text.trim().is_empty());
        }
        Ok(())
    })
}

fn parse_defaults<S: Source>(json: &mut Reader<S>, raw: &mut RawPanel) -> Result<()> {
    json.object(|json, key| {
        match key {
            "unit" => raw.unit = json.optional_string(32)?,
            "decimals" => raw.decimals = json.optional_number()?,
            "thresholds" if json.kind()? == JsonKind::Object => json.object(|json, key| {
                if key != "steps" || json.kind()? != JsonKind::Array {
                    return json.skip();
                }
                json.array(|json| {
                    if json.kind()? != JsonKind::Object {
                        return json.skip();
                    }
                    json.object(|json, key| {
                        if key == "value" {
                            // The base step has no value.
                            if let Some(value) = json.optional_number()?
                                && raw.thresholds.len() < 2
                            {
                                raw.thresholds.push(value);
                            }
                            Ok(())
                        } else {
                            json.skip()
                        }
                    })
                })
            })?,
            _ => json.skip()?,
        }
        Ok(())
    })
}

/// Reads one panel; a row's nested panels are added to `collected` first.
fn parse_panel<S: Source>(
    json: &mut Reader<S>,
    collected: &mut Collected,
    nested: bool,
) -> Result<()> {
    if json.kind()? != JsonKind::Object {
        return json.skip();
    }
    let mut raw = RawPanel::default();
    json.object(|json, key| {
        match key {
            "type" => raw.kind = json.optional_string(32)?.unwrap_or_default(),
            "title" => {
                if json.kind()? == JsonKind::String {
                    raw.title = json.truncated_string(MAXIMUM_TITLE_LENGTH + 4)?;
                } else {
                    json.skip()?;
                }
            }
            "id" => raw.id = json.optional_number()?,
            "targets" if json.kind()? == JsonKind::Array => parse_targets(json, &mut raw)?,
            "fieldConfig" if json.kind()? == JsonKind::Object => json.object(|json, key| {
                if key == "defaults" && json.kind()? == JsonKind::Object {
                    parse_defaults(json, &mut raw)
                } else {
                    json.skip()
                }
            })?,
            // Collapsed rows hold their panels; rows do not nest.
            "panels" if !nested && json.kind()? == JsonKind::Array => {
                json.array(|json| parse_panel(json, collected, true))?
            }
            _ => json.skip()?,
        }
        Ok(())
    })?;
    collected.add(raw);
    Ok(())
}

/// Substitution for one template variable.
#[derive(Debug, Clone, PartialEq)]
struct Variable {
    name: String,
    value: String,
}

/// Reads `current.value` and the "all" value of one variable.
fn parse_variable<S: Source>(json: &mut Reader<S>) -> Result<Option<Variable>> {
    if json.kind()? != JsonKind::Object {
        json.skip()?;
        return Ok(None);
    }
    let mut name = None;
    let mut values: Option<Vec<String>> = None;
    let mut all_value = None;
    let mut too_many = false;
    json.object(|json, key| {
        match key {
            "name" => name = json.optional_string(64)?,
            "allValue" => all_value = json.optional_string(MAXIMUM_VARIABLE_LENGTH)?,
            "current" if json.kind()? == JsonKind::Object => json.object(|json, key| {
                if key != "value" {
                    return json.skip();
                }
                match json.kind()? {
                    JsonKind::String => {
                        values = Some(std::vec![json.truncated_string(MAXIMUM_VARIABLE_LENGTH)?]);
                    }
                    JsonKind::Array => {
                        let mut list = Vec::new();
                        json.array(|json| {
                            // A value too long to keep makes the whole
                            // variable unusable, like too many values.
                            match json.optional_string(MAXIMUM_VARIABLE_LENGTH) {
                                Ok(Some(value)) if list.len() < MAXIMUM_VARIABLE_VALUES => {
                                    list.push(value);
                                }
                                Ok(Some(_)) | Err(Error::TooLarge) => too_many = true,
                                Ok(None) => (),
                                Err(error) => return Err(error),
                            }
                            Ok(())
                        })?;
                        values = Some(list);
                    }
                    _ => json.skip()?,
                }
                Ok(())
            })?,
            _ => json.skip()?,
        }
        Ok(())
    })?;
    let (Some(name), Some(values)) = (name, values) else {
        return Ok(None);
    };
    if too_many {
        return Ok(None);
    }
    let all = || {
        all_value
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| String::from(".*"))
    };
    let value = if values.is_empty()
        || values
            .iter()
            .any(|value| value == "$__all" || value == "All")
    {
        all()
    } else {
        values.join("|")
    };
    if value.len() > MAXIMUM_VARIABLE_LENGTH {
        return Ok(None);
    }
    Ok(Some(Variable { name, value }))
}

/// Replaces `$name`, `${name}`, `${name:format}` and `[[name]]` with the
/// variable values and Grafana's interval macros with [`INTERVAL`]. Unknown
/// names, such as `$1` in `label_replace`, are left as they are.
fn substitute(expression: &str, variables: &[Variable]) -> String {
    let lookup = |name: &str| -> Option<&str> {
        match name {
            "__rate_interval" | "__interval" | "__range" => Some(INTERVAL),
            _ => variables
                .iter()
                .find(|variable| variable.name == name)
                .map(|variable| variable.value.as_str()),
        }
    };
    let identifier_length = |text: &str| {
        text.bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            .count()
    };
    let mut output = String::with_capacity(expression.len());
    let mut rest = expression;
    while let Some(position) = rest.find(['$', '[']) {
        output.push_str(&rest[..position]);
        rest = &rest[position..];
        let (name, length) = if let Some(inner) = rest.strip_prefix("${") {
            match inner.find('}') {
                Some(end) => {
                    let name = inner[..end].split(':').next().unwrap_or("");
                    (name, end + 3)
                }
                None => ("", 0),
            }
        } else if let Some(inner) = rest.strip_prefix("[[") {
            match inner.find("]]") {
                Some(end) => (&inner[..end], end + 4),
                None => ("", 0),
            }
        } else if let Some(inner) = rest.strip_prefix('$') {
            let length = identifier_length(inner);
            (&inner[..length], length + 1)
        } else {
            ("", 0)
        };
        match lookup(name).filter(|_| length > 0) {
            Some(value) => {
                output.push_str(value);
                rest = &rest[length..];
            }
            None => {
                output.push_str(&rest[..1]);
                rest = &rest[1..];
            }
        }
    }
    output.push_str(rest);
    output
}

/// Seconds in a relative Grafana time such as `now-24h`, if it is one.
fn relative_seconds(from: &str) -> Option<u32> {
    let amount = from.strip_prefix("now-")?;
    let amount = amount.split('/').next()?;
    let split = amount.find(|character: char| !character.is_ascii_digit())?;
    let (number, unit) = amount.split_at(split);
    let number: u64 = number.parse().ok()?;
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        "M" => 2_592_000,
        "y" => 31_536_000,
        _ => return None,
    };
    u32::try_from(number.saturating_mul(scale)).ok()
}

/// Converts the dashboard read from `json`, the body of
/// `GET /api/dashboards/uid/<uid>`.
pub fn parse_dashboard<S: Source>(json: &mut Reader<S>, uid: &str) -> Result<Imported> {
    let mut collected = Collected::default();
    let mut variables = Vec::new();
    let mut title = String::new();
    let mut range = None;
    let mut found = false;
    json.object(|json, key| {
        if key != "dashboard" || json.kind()? != JsonKind::Object {
            return json.skip();
        }
        found = true;
        json.object(|json, key| {
            match key {
                "title" => {
                    if json.kind()? == JsonKind::String {
                        title = json.truncated_string(MAXIMUM_TITLE_LENGTH)?;
                    } else {
                        json.skip()?;
                    }
                }
                "panels" if json.kind()? == JsonKind::Array => {
                    json.array(|json| parse_panel(json, &mut collected, false))?
                }
                "templating" if json.kind()? == JsonKind::Object => json.object(|json, key| {
                    if key != "list" || json.kind()? != JsonKind::Array {
                        return json.skip();
                    }
                    json.array(|json| {
                        if let Some(variable) = parse_variable(json)?
                            && variables.len() < MAXIMUM_VARIABLES
                        {
                            variables.push(variable);
                        }
                        Ok(())
                    })
                })?,
                "time" if json.kind()? == JsonKind::Object => json.object(|json, key| {
                    if key == "from" {
                        range = json
                            .optional_string(32)?
                            .as_deref()
                            .and_then(relative_seconds);
                        Ok(())
                    } else {
                        json.skip()
                    }
                })?,
                _ => json.skip()?,
            }
            Ok(())
        })
    })?;
    json.finish()?;
    if !found {
        return Err(Error::InvalidResponse);
    }
    let range = range
        .unwrap_or(DEFAULT_RANGE_SECONDS)
        .clamp(60, MAXIMUM_RANGE_SECONDS);
    let mut skipped = collected.skipped;
    let mut panels = Vec::with_capacity(collected.panels.len());
    for (mut panel, kind) in collected.panels {
        panel.expression = substitute(&panel.expression, &variables);
        // Substituted values can make an expression longer than the limit
        // the settings enforce: such a panel is skipped.
        if panel.expression.len() > MAXIMUM_EXPRESSION_LENGTH {
            skipped += 1;
            continue;
        }
        if kind == Kind::Chart {
            panel.range_seconds = Some(range);
        }
        panels.push(panel);
    }
    Ok(Imported {
        uid: String::from(uid),
        title,
        skipped,
        panels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::SliceSource;

    fn variables() -> Vec<Variable> {
        std::vec![
            Variable {
                name: String::from("host"),
                value: String::from(".*")
            },
            Variable {
                name: String::from("job"),
                value: String::from("node")
            },
            Variable {
                name: String::from("instance"),
                value: String::from("a|b")
            },
        ]
    }

    #[test]
    fn substitutes_variables_and_macros() {
        let variables = variables();
        assert_eq!(
            substitute(
                r#"rate(x{host=~"$host", job="${job}", instance=~"[[instance]]"}[$__rate_interval])"#,
                &variables
            ),
            r#"rate(x{host=~".*", job="node", instance=~"a|b"}[5m])"#
        );
        assert_eq!(
            substitute("${job:regex} $__interval $__range", &variables),
            "node 5m 5m"
        );
        // Unknown names, regular expression groups and partial names stay.
        assert_eq!(
            substitute(
                r#"label_replace(x, "a", "$1", "b", "(.*)") $hostname $__range_s $ [x] ${"#,
                &variables
            ),
            r#"label_replace(x, "a", "$1", "b", "(.*)") $hostname $__range_s $ [x] ${"#
        );
    }

    #[test]
    fn relative_ranges() {
        assert_eq!(relative_seconds("now-24h"), Some(86_400));
        assert_eq!(relative_seconds("now-15m"), Some(900));
        assert_eq!(relative_seconds("now-7d/d"), Some(604_800));
        assert_eq!(relative_seconds("2026-01-01"), None);
        assert_eq!(relative_seconds("now-h"), None);
    }

    #[test]
    fn converts_the_fixture_dashboard() {
        let text = include_str!("../tests/fixtures/dashboard.json");
        let imported = parse_dashboard(
            &mut Reader::new(SliceSource::new(text.as_bytes())),
            "servers",
        )
        .unwrap();
        assert_eq!(imported.uid, "servers");
        assert_eq!(imported.title, "Servers");
        // Table, pie chart, text and a panel without an expression.
        assert_eq!(imported.skipped, 4);
        let titles: Vec<&str> = imported
            .panels
            .iter()
            .map(|panel| panel.title.as_str())
            .collect();
        assert_eq!(
            titles,
            [
                "CPU",
                "Memory",
                "Uptime",
                "CPU over time",
                "Root free space",
                "Temperature"
            ]
        );

        let cpu = &imported.panels[0];
        assert_eq!(cpu.key.as_deref(), Some("2"));
        assert_eq!(cpu.kind(), Kind::Value);
        assert_eq!(cpu.unit(), Unit::Fraction);
        assert_eq!(cpu.decimals, Some(1));
        assert_eq!((cpu.warning, cpu.critical), (Some(0.8), Some(0.9)));
        assert_eq!(
            cpu.expression,
            r#"avg by (host) (rate(node_cpu_seconds_total{host=~".*", mode!="idle"}[5m]))"#
        );

        let memory = &imported.panels[1];
        assert_eq!(memory.unit(), Unit::Percent);
        assert_eq!((memory.warning, memory.critical), (Some(80.0), None));
        assert_eq!(
            memory.expression,
            r#"node_memory_used_percent{job="node", instance=~"a|b"}"#
        );

        let uptime = &imported.panels[2];
        assert_eq!(uptime.unit(), Unit::Seconds);
        assert_eq!(uptime.decimals, None);
        assert_eq!((uptime.warning, uptime.critical), (None, None));

        let chart = &imported.panels[3];
        assert_eq!(chart.kind(), Kind::Chart);
        assert_eq!(chart.range_seconds, Some(86_400), "capped at a day");
        // The hidden first target is ignored.
        assert!(chart.expression.starts_with("sum(rate("));

        // Panels of the collapsed row are kept, with their units.
        assert_eq!(imported.panels[4].unit(), Unit::Bytes);
        assert_eq!(imported.panels[5].unit(), Unit::Celsius);
        assert_eq!(imported.panels[5].kind(), Kind::Value);
        assert_eq!(
            imported.panels[5].expression,
            "max(node_hwmon_temp_celsius{host=~\".*\"})"
        );
    }

    #[test]
    fn bounds_variables_and_substituted_expressions() {
        let dashboard = |variable: &str, expression: &str| {
            std::format!(
                r#"{{"dashboard": {{"title": "T", "templating": {{"list": [{variable}]}},
                "panels": [{{"type": "stat", "id": 1, "title": "P", "targets": [{{"expr": "{expression}"}}]}}]}}}}"#
            )
        };
        let import = |text: String| {
            parse_dashboard(&mut Reader::new(SliceSource::new(text.as_bytes())), "x").unwrap()
        };
        // Too many selected values: the variable is left alone.
        let values: Vec<String> = (0..40).map(|index| std::format!("\"v{index}\"")).collect();
        let many = std::format!(
            r#"{{"name": "v", "current": {{"value": [{}]}}}}"#,
            values.join(",")
        );
        assert_eq!(
            import(dashboard(&many, r#"up{h=~\"$v\"}"#)).panels[0].expression,
            r#"up{h=~"$v"}"#
        );
        // A value of 200 bytes repeated 20 times passes the expression limit.
        let long = "x".repeat(200);
        let variable = std::format!(r#"{{"name": "v", "current": {{"value": "{long}"}}}}"#);
        let expression = "$v ".repeat(20);
        let imported = import(dashboard(&variable, &expression));
        assert!(imported.panels.is_empty());
        assert_eq!(imported.skipped, 1);
        // The same expression with a short value stays.
        let short = r#"{"name": "v", "current": {"value": "a"}}"#;
        assert_eq!(import(dashboard(short, &expression)).panels.len(), 1);
    }

    #[test]
    fn rejects_other_documents() {
        let mut json = Reader::new(SliceSource::new(br#"{"message": "Dashboard not found"}"#));
        assert!(matches!(
            parse_dashboard(&mut json, "x"),
            Err(Error::InvalidResponse)
        ));
    }
}
