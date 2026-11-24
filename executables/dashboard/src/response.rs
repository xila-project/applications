//! Streaming parser for `/api/ds/query` responses.
//!
//! The response holds one result per query reference, each with data frames
//! (one per series for Prometheus):
//!
//! ```json
//! {"results": {"P0": {"status": 200, "frames": [{
//!     "schema": {"fields": [{"name": "Time", "type": "time"},
//!                           {"name": "Value", "type": "number", "labels": {"host": "a"}}]},
//!     "data": {"values": [[1700000000000, …], [0.42, …]]}}]}}}
//! ```
//!
//! Values are read one at a time: a series keeps its last value and at most
//! [`CHART_POINTS`](crate::query::CHART_POINTS) averaged points.

use crate::configuration::Kind;
use crate::error::Result;
use crate::json::{Kind as JsonKind, Reader, Source};
use crate::query::{CHART_POINTS, panel_index};
use std::{string::String, vec::Vec};

/// Series kept per panel.
pub const MAXIMUM_SERIES: usize = 3;
/// Longest series name kept.
const MAXIMUM_NAME_LENGTH: usize = 48;
/// Longest error message kept.
const MAXIMUM_ERROR_LENGTH: usize = 120;

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// Label values, or the legend Grafana computed; empty for a lone series.
    pub name: String,
    /// Last value that is not null.
    pub last: Option<f64>,
    /// Chart points, oldest first; empty for value panels.
    pub points: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelData {
    pub series: Vec<Series>,
    /// Series left out because the panel already shows the most it can.
    pub more: usize,
    /// Error Grafana reported for the query.
    pub error: Option<String>,
}

impl PanelData {
    /// Keeps `series` according to the panel kind: value panels keep the
    /// first series, chart panels the ones with the highest last value.
    fn add(&mut self, series: Series, kind: Kind) {
        match kind {
            Kind::Value => {
                if self.series.len() < MAXIMUM_SERIES {
                    self.series.push(series);
                } else {
                    self.more += 1;
                }
            }
            Kind::Chart => {
                self.series.push(series);
                if self.series.len() > MAXIMUM_SERIES {
                    let lowest = self
                        .series
                        .iter()
                        .enumerate()
                        .min_by(|(_, a), (_, b)| order(a.last, b.last))
                        .map(|(index, _)| index)
                        .unwrap_or(0);
                    self.series.remove(lowest);
                    self.more += 1;
                }
            }
        }
    }

    fn finish(&mut self, kind: Kind) {
        if kind == Kind::Chart {
            self.series.sort_by(|a, b| order(b.last, a.last));
        }
    }
}

/// Orders optional values, `None` first.
fn order(a: Option<f64>, b: Option<f64>) -> core::cmp::Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(core::cmp::Ordering::Equal),
        (a, b) => a.is_some().cmp(&b.is_some()),
    }
}

/// Averages incoming values into at most `capacity` points, whatever their
/// number: when the points are full, neighbours are merged in pairs and each
/// new point averages twice as many values.
pub struct Downsampler {
    points: Vec<f32>,
    capacity: usize,
    stride: usize,
    sum: f64,
    count: usize,
}

impl Downsampler {
    pub fn new(capacity: usize) -> Self {
        Self {
            points: Vec::new(),
            // Merging pairs needs an even capacity.
            capacity: capacity & !1,
            stride: 1,
            sum: 0.0,
            count: 0,
        }
    }

    pub fn push(&mut self, value: f64) {
        if self.capacity == 0 {
            return;
        }
        self.sum += value;
        self.count += 1;
        if self.count < self.stride {
            return;
        }
        if self.points.len() == self.capacity {
            // Full: merge neighbours, and let the current bucket grow to the
            // new stride instead of becoming a point.
            self.merge();
            self.stride *= 2;
            return;
        }
        self.points.push((self.sum / self.count as f64) as f32);
        self.sum = 0.0;
        self.count = 0;
    }

    pub fn finish(mut self) -> Vec<f32> {
        if self.count > 0 {
            if self.points.len() == self.capacity {
                self.merge();
            }
            self.points.push((self.sum / self.count as f64) as f32);
        }
        self.points
    }

    /// Merges neighbouring points in pairs.
    fn merge(&mut self) {
        for index in 0..self.capacity / 2 {
            self.points[index] = (self.points[2 * index] + self.points[2 * index + 1]) / 2.0;
        }
        self.points.truncate(self.capacity / 2);
    }
}

/// What the schema says about a column.
struct Field {
    time: bool,
    name: String,
}

fn parse_field<S: Source>(json: &mut Reader<S>) -> Result<Field> {
    let mut name = String::new();
    let mut labels = String::new();
    let mut display = None;
    let mut time = false;
    json.object(|json, key| {
        match key {
            "name" => name = json.truncated_string(MAXIMUM_NAME_LENGTH)?,
            "type" => time = json.truncated_string(16)? == "time",
            "labels" if json.kind()? == JsonKind::Object => json.object(|json, _| {
                if let Some(value) = json.optional_string(MAXIMUM_NAME_LENGTH)?
                    && labels.len() < MAXIMUM_NAME_LENGTH
                {
                    if !labels.is_empty() {
                        labels.push(' ');
                    }
                    labels.push_str(&value);
                }
                Ok(())
            })?,
            "config" if json.kind()? == JsonKind::Object => json.object(|json, key| {
                if key == "displayNameFromDS" && json.kind()? == JsonKind::String {
                    display = Some(json.truncated_string(MAXIMUM_NAME_LENGTH)?);
                    Ok(())
                } else {
                    json.skip()
                }
            })?,
            _ => json.skip()?,
        }
        Ok(())
    })?;
    let name = match display {
        Some(display) if !display.is_empty() => display,
        _ if !labels.is_empty() => labels,
        // A lone value column needs no name.
        _ if name == "Value" => String::new(),
        _ => name,
    };
    let mut name = name;
    if name.len() > MAXIMUM_NAME_LENGTH {
        let mut end = MAXIMUM_NAME_LENGTH;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    Ok(Field { time, name })
}

fn parse_frame<S: Source>(json: &mut Reader<S>, data: &mut PanelData, kind: Kind) -> Result<()> {
    let mut fields: Option<Vec<Field>> = None;
    json.object(|json, key| match key {
        "schema" => json.object(|json, key| {
            if key == "fields" {
                let mut list = Vec::new();
                json.array(|json| {
                    list.push(parse_field(json)?);
                    Ok(())
                })?;
                fields = Some(list);
                Ok(())
            } else {
                json.skip()
            }
        }),
        "data" => json.object(|json, key| {
            if key != "values" {
                return json.skip();
            }
            let mut column = 0;
            json.array(|json| {
                let field = fields.as_ref().and_then(|fields| fields.get(column));
                // Without a schema, the first column is the time.
                let time = field.map_or(column == 0, |field| field.time);
                column += 1;
                if time {
                    return json.skip();
                }
                let capacity = match kind {
                    Kind::Value => 0,
                    Kind::Chart => CHART_POINTS as usize,
                };
                let mut points = Downsampler::new(capacity);
                let mut last = None;
                json.array(|json| {
                    if let Some(value) = json.optional_number()? {
                        last = Some(value);
                        points.push(value);
                    }
                    Ok(())
                })?;
                data.add(
                    Series {
                        name: field.map(|field| field.name.clone()).unwrap_or_default(),
                        last,
                        points: points.finish(),
                    },
                    kind,
                );
                Ok(())
            })
        }),
        _ => json.skip(),
    })
}

fn parse_result<S: Source>(json: &mut Reader<S>, data: &mut PanelData, kind: Kind) -> Result<()> {
    json.object(|json, key| match key {
        "error" => {
            if json.kind()? == JsonKind::String {
                let error = json.truncated_string(MAXIMUM_ERROR_LENGTH)?;
                if !error.is_empty() {
                    data.error = Some(error);
                }
                Ok(())
            } else {
                json.skip()
            }
        }
        "frames" if json.kind()? == JsonKind::Array => {
            json.array(|json| parse_frame(json, data, kind))
        }
        _ => json.skip(),
    })?;
    data.finish(kind);
    Ok(())
}

/// Reads a response into `data`, indexed like the panels whose kinds are
/// `kinds`. Results for unknown references are ignored.
pub fn parse<S: Source>(
    json: &mut Reader<S>,
    kinds: &[Kind],
    data: &mut [PanelData],
) -> Result<()> {
    json.object(|json, key| {
        if key != "results" {
            return json.skip();
        }
        json.object(|json, reference| {
            match panel_index(reference).filter(|index| *index < kinds.len().min(data.len())) {
                Some(index) => parse_result(json, &mut data[index], kinds[index]),
                None => json.skip(),
            }
        })
    })?;
    json.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::json::SliceSource;

    fn parse_text(text: &str, kinds: &[Kind]) -> Result<Vec<PanelData>> {
        let mut data = std::vec![PanelData::default(); kinds.len()];
        parse(
            &mut Reader::new(SliceSource::new(text.as_bytes())),
            kinds,
            &mut data,
        )?;
        Ok(data)
    }

    #[test]
    fn instant_response_keeps_the_first_three_series() {
        let data = parse_text(
            include_str!("../tests/fixtures/instant.json"),
            &[Kind::Value, Kind::Value],
        )
        .unwrap();
        let cpu = &data[0];
        assert_eq!(cpu.error, None);
        assert_eq!(cpu.more, 1);
        let names: Vec<&str> = cpu
            .series
            .iter()
            .map(|series| series.name.as_str())
            .collect();
        assert_eq!(names, ["alpha", "beta", "gamma"]);
        assert_eq!(cpu.series[0].last, Some(0.25));
        assert!(cpu.series[0].points.is_empty());
        // A failed query reports Grafana's message.
        assert_eq!(data[1].series, []);
        assert_eq!(
            data[1].error.as_deref(),
            Some("bad_data: invalid parameter \"query\": parse error")
        );
    }

    #[test]
    fn range_response_keeps_the_top_three_and_downsamples() {
        let data = parse_text(
            include_str!("../tests/fixtures/range.json"),
            &[Kind::Value, Kind::Chart],
        )
        .unwrap();
        assert_eq!(data[0], PanelData::default(), "no result for P0");
        let chart = &data[1];
        assert_eq!(chart.more, 1);
        let summary: Vec<(&str, Option<f64>)> = chart
            .series
            .iter()
            .map(|series| (series.name.as_str(), series.last))
            .collect();
        assert_eq!(
            summary,
            [
                ("web", Some(119.0)),
                ("db", Some(30.0)),
                ("cache", Some(20.0))
            ]
        );
        // 120 points, then a null: halved once to 60.
        assert_eq!(chart.series[0].points.len(), 60);
        assert_eq!(chart.series[0].points[0], 0.5);
        assert_eq!(chart.series[0].points[59], 118.5);
        // Nulls are skipped; the last value ignores them.
        assert_eq!(chart.series[1].points, [10.0, 30.0]);
    }

    #[test]
    fn frames_without_schema_and_odd_shapes() {
        let data = parse_text(
            r#"{"results": {"P0": {"frames": [{"data": {"values": [[1, 2], [5, 6]]}}]}, "Q": {"x": 1}}, "extra": []}"#,
            &[Kind::Value],
        )
        .unwrap();
        assert_eq!(data[0].series[0].last, Some(6.0));
        assert_eq!(data[0].series[0].name, "");
        assert_eq!(
            parse_text("{\"results\": [}", &[Kind::Value]),
            Err(Error::InvalidResponse)
        );
        assert_eq!(parse_text("{} x", &[]), Err(Error::InvalidResponse));
    }

    #[test]
    fn downsampler_bounds_points_for_any_length() {
        for count in [0, 1, 59, 60, 61, 119, 120, 121, 1_000, 10_000] {
            let mut sampler = Downsampler::new(60);
            for value in 0..count {
                sampler.push(f64::from(value));
            }
            let points = sampler.finish();
            assert!(
                points.len() <= 60,
                "{count} values gave {} points",
                points.len()
            );
            if count >= 60 {
                assert!(
                    points.len() >= 30,
                    "{count} values gave {} points",
                    points.len()
                );
            } else {
                assert_eq!(points.len(), count as usize);
            }
            // Averages keep the series increasing.
            assert!(points.windows(2).all(|pair| pair[0] < pair[1]));
        }
        let mut none = Downsampler::new(0);
        none.push(1.0);
        assert!(none.finish().is_empty());
    }
}
