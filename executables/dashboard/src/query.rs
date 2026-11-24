//! Requests for Grafana's query endpoint, `POST /api/ds/query`.
//!
//! Every panel becomes one query, with reference `P<index>`. Value panels are
//! instant queries; chart panels are range queries of about [`CHART_POINTS`]
//! points. Grafana applies one time range to a whole request, so panels are
//! grouped by range, and a group is split when its body gets large.

use crate::configuration::{Kind, Panel};
use crate::json::push_string;
use std::{format, string::String, vec::Vec};

/// Points asked for, and kept, per chart series.
pub const CHART_POINTS: u32 = 60;
/// A request body grows past this size only for a single huge expression.
pub const MAXIMUM_BODY_LENGTH: usize = 8 * 1024;

/// One request: the panels it answers and its JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub panels: Vec<usize>,
    pub body: String,
}

/// Reference of panel `index` in requests and responses.
pub fn reference(index: usize) -> String {
    format!("P{index}")
}

/// Panel index of a reference, if it is one of ours.
pub fn panel_index(reference: &str) -> Option<usize> {
    let digits = reference.strip_prefix('P')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn push_query(body: &mut String, index: usize, panel: &Panel, data_source: &str) {
    let range = u64::from(panel.range_seconds());
    body.push_str("{\"refId\":");
    push_string(body, &reference(index));
    body.push_str(",\"datasource\":{\"uid\":");
    push_string(body, data_source);
    body.push_str("},\"expr\":");
    push_string(body, &panel.expression);
    match panel.kind() {
        Kind::Value => body.push_str(",\"instant\":true,\"range\":false"),
        Kind::Chart => body.push_str(&format!(
            ",\"instant\":false,\"range\":true,\"intervalMs\":{},\"maxDataPoints\":{CHART_POINTS}",
            (range * 1_000 / u64::from(CHART_POINTS)).max(1_000)
        )),
    }
    body.push('}');
}

fn body(range_seconds: u32, queries: &str) -> String {
    format!("{{\"from\":\"now-{range_seconds}s\",\"to\":\"now\",\"queries\":[{queries}]}}")
}

/// Range used to group a panel: instant queries go with the first group.
fn group_range(panel: &Panel) -> Option<u32> {
    match panel.kind() {
        Kind::Value => None,
        Kind::Chart => Some(panel.range_seconds()),
    }
}

/// Builds the requests answering `panels`, in as few batches as possible.
pub fn batches(panels: &[&Panel], data_source: &str) -> Vec<Batch> {
    // Distinct ranges in order of first use; instant queries carry no range.
    let mut ranges: Vec<Option<u32>> = Vec::new();
    for panel in panels {
        let range = group_range(panel);
        if !ranges.contains(&range) {
            ranges.push(range);
        }
    }
    // Instant queries join the first range group, if there is one.
    let instant_group = ranges.iter().copied().flatten().next();
    let mut batches = Vec::new();
    let mut groups: Vec<u32> = ranges.iter().copied().flatten().collect();
    if groups.is_empty() {
        groups.push(crate::configuration::DEFAULT_RANGE_SECONDS);
    }
    for range in groups {
        let mut indices = Vec::new();
        let mut queries = String::new();
        for (index, panel) in panels.iter().enumerate() {
            let belongs = match group_range(panel) {
                Some(panel_range) => panel_range == range,
                None => instant_group.is_none_or(|first| first == range),
            };
            if !belongs {
                continue;
            }
            let mut query = String::new();
            push_query(&mut query, index, panel, data_source);
            if !indices.is_empty() && queries.len() + query.len() + 1 > MAXIMUM_BODY_LENGTH {
                batches.push(Batch {
                    panels: core::mem::take(&mut indices),
                    body: body(range, &queries),
                });
                queries.clear();
            }
            if !queries.is_empty() {
                queries.push(',');
            }
            queries.push_str(&query);
            indices.push(index);
        }
        if !indices.is_empty() {
            batches.push(Batch {
                panels: indices,
                body: body(range, &queries),
            });
        }
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::{Reader, SliceSource};

    fn panel(expression: &str, kind: Kind, range_seconds: Option<u32>) -> Panel {
        Panel {
            title: String::from("Panel"),
            expression: String::from(expression),
            kind: Some(kind),
            range_seconds,
            ..Panel::new()
        }
    }

    #[test]
    fn references_round_trip() {
        assert_eq!(panel_index(&reference(12)), Some(12));
        assert_eq!(panel_index("A"), None);
        assert_eq!(panel_index("P"), None);
        assert_eq!(panel_index("P1x"), None);
    }

    #[test]
    fn instant_queries_share_one_request() {
        let first = panel("up{job=\"node\"}", Kind::Value, None);
        let second = panel("node_load1", Kind::Value, None);
        let batches = batches(&[&first, &second], "prometheus");
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].panels, [0, 1]);
        assert_eq!(
            batches[0].body,
            r#"{"from":"now-3600s","to":"now","queries":[{"refId":"P0","datasource":{"uid":"prometheus"},"expr":"up{job=\"node\"}","instant":true,"range":false},{"refId":"P1","datasource":{"uid":"prometheus"},"expr":"node_load1","instant":true,"range":false}]}"#
        );
        // The body is valid JSON.
        Reader::new(SliceSource::new(batches[0].body.as_bytes()))
            .skip()
            .unwrap();
    }

    #[test]
    fn range_queries_group_by_range_and_ask_for_sixty_points() {
        let value = panel("a", Kind::Value, None);
        let day = panel("b", Kind::Chart, Some(86_400));
        let hour = panel("c", Kind::Chart, Some(3_600));
        let other_day = panel("d", Kind::Chart, Some(86_400));
        let batches = batches(&[&value, &day, &hour, &other_day], "uid");
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].panels, [0, 1, 3]);
        assert!(batches[0].body.starts_with(r#"{"from":"now-86400s""#));
        assert!(
            batches[0]
                .body
                .contains(r#""intervalMs":1440000,"maxDataPoints":60"#)
        );
        assert_eq!(batches[1].panels, [2]);
        assert!(batches[1].body.contains(r#""intervalMs":60000"#));
    }

    #[test]
    fn large_groups_are_split() {
        let long = "x".repeat(3_000);
        let panels: Vec<Panel> = (0..5).map(|_| panel(&long, Kind::Value, None)).collect();
        let references: Vec<&Panel> = panels.iter().collect();
        let batches = batches(&references, "uid");
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].panels, [0, 1]);
        assert_eq!(batches[2].panels, [4]);
        assert!(
            batches
                .iter()
                .all(|batch| batch.body.len() < MAXIMUM_BODY_LENGTH + 200)
        );
    }
}
