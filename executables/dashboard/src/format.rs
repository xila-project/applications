//! Value formatting by unit, and threshold levels.
//!
//! Numbers are formatted with integer arithmetic so that the guest does not
//! pull `core::fmt::float` in.

use miniserde::{Deserialize, Serialize};
use std::{format, string::String};

/// Unit of a panel value. Names follow the configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Unit {
    #[default]
    #[serde(rename = "none")]
    None,
    /// A value from 0 to 100, shown with `%`.
    #[serde(rename = "percent")]
    Percent,
    /// A value from 0 to 1, shown as a percentage (Grafana's `percentunit`).
    #[serde(rename = "fraction")]
    Fraction,
    /// Bytes, with binary prefixes (KiB, MiB…).
    #[serde(rename = "bytes")]
    Bytes,
    /// Bytes per second, with decimal prefixes (kB/s, MB/s…).
    #[serde(rename = "bytes_per_second")]
    BytesPerSecond,
    /// A duration in seconds.
    #[serde(rename = "seconds")]
    Seconds,
    #[serde(rename = "celsius")]
    Celsius,
}

impl Unit {
    /// Every unit, in the order of the settings drop-down.
    pub const ALL: [Unit; 7] = [
        Unit::None,
        Unit::Percent,
        Unit::Fraction,
        Unit::Bytes,
        Unit::BytesPerSecond,
        Unit::Seconds,
        Unit::Celsius,
    ];

    /// Unit for a Grafana unit identifier; unknown ones become [`Unit::None`].
    pub fn from_grafana(identifier: &str) -> Self {
        match identifier {
            "percent" => Unit::Percent,
            "percentunit" => Unit::Fraction,
            "bytes" | "decbytes" => Unit::Bytes,
            "Bps" | "binBps" => Unit::BytesPerSecond,
            "s" => Unit::Seconds,
            "celsius" => Unit::Celsius,
            _ => Unit::None,
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|unit| *unit == self).unwrap_or(0)
    }
}

/// The largest number of fractional digits [`format_fixed`] supports, so the
/// scaled value always fits in 64 bits.
pub const MAXIMUM_DECIMALS: u8 = 6;

/// Formats `value` with `decimals` fractional digits, rounding half away from
/// zero. A value that is not finite or too large is shown as zero.
pub fn format_fixed(value: f64, decimals: u8) -> String {
    let decimals = decimals.min(MAXIMUM_DECIMALS);
    let divisor = 10_u64.pow(u32::from(decimals));
    let scaled = value * divisor as f64;
    // `round` is not in `core`: add a half and truncate towards zero.
    let scaled = if scaled < 0.0 {
        scaled - 0.5
    } else {
        scaled + 0.5
    };
    let rounded = if scaled.abs() < i64::MAX as f64 {
        scaled as i64
    } else {
        0
    };
    let sign = if rounded < 0 { "-" } else { "" };
    let magnitude = rounded.unsigned_abs();
    let whole = magnitude / divisor;
    if decimals == 0 {
        format!("{sign}{whole}")
    } else {
        let fraction = magnitude % divisor;
        let width = usize::from(decimals);
        format!("{sign}{whole}.{fraction:0width$}")
    }
}

/// Formats with automatic precision: three significant digits at most, and
/// no trailing zeros.
fn format_automatic(value: f64) -> String {
    let magnitude = value.abs();
    let decimals = if magnitude >= 100.0 {
        0
    } else if magnitude >= 10.0 {
        1
    } else if magnitude >= 1.0 {
        2
    } else {
        3
    };
    let mut text = format_fixed(value, decimals);
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    if text == "-0" {
        text.remove(0);
    }
    text
}

fn number(value: f64, decimals: Option<u8>) -> String {
    match decimals {
        Some(decimals) => format_fixed(value, decimals),
        None => format_automatic(value),
    }
}

/// Divides `value` by `step` until it is below `step`, and returns the
/// scaled value with the matching prefix.
fn scaled(value: f64, step: f64, prefixes: &[&'static str]) -> (f64, &'static str) {
    let mut value = value;
    let mut index = 0;
    while value.abs() >= step && index + 1 < prefixes.len() {
        value /= step;
        index += 1;
    }
    (value, prefixes[index])
}

fn duration(value: f64, decimals: Option<u8>) -> String {
    const STEPS: [(f64, &str); 6] = [
        (60.0, "min"),
        (3_600.0, "h"),
        (86_400.0, "d"),
        (604_800.0, "w"),
        (31_536_000.0, "y"),
        (f64::INFINITY, ""),
    ];
    let magnitude = value.abs();
    if magnitude != 0.0 && magnitude < 1.0 {
        return format!("{} ms", number(value * 1_000.0, decimals));
    }
    if magnitude < 60.0 {
        return format!("{} s", number(value, decimals));
    }
    for window in STEPS.windows(2) {
        let ((size, name), (next, _)) = (window[0], window[1]);
        if magnitude < next {
            return format!("{} {name}", number(value / size, decimals));
        }
    }
    unreachable!("the last step is infinite")
}

/// Formats `value` in `unit`, with fixed `decimals` or automatic precision.
pub fn format_value(value: f64, unit: Unit, decimals: Option<u8>) -> String {
    if !value.is_finite() {
        return String::from("-");
    }
    match unit {
        Unit::None => number(value, decimals),
        Unit::Percent => format!("{}%", number(value, decimals)),
        Unit::Fraction => format!("{}%", number(value * 100.0, decimals)),
        Unit::Bytes => {
            let (value, prefix) = scaled(value, 1024.0, &["B", "KiB", "MiB", "GiB", "TiB", "PiB"]);
            format!("{} {prefix}", number(value, decimals))
        }
        Unit::BytesPerSecond => {
            let (value, prefix) = scaled(
                value,
                1000.0,
                &["B/s", "kB/s", "MB/s", "GB/s", "TB/s", "PB/s"],
            );
            format!("{} {prefix}", number(value, decimals))
        }
        Unit::Seconds => duration(value, decimals),
        Unit::Celsius => format!("{} °C", number(value, decimals)),
    }
}

/// How a value compares with its panel's thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// No threshold is set.
    Neutral,
    Normal,
    Warning,
    Critical,
}

impl Level {
    /// Text colour, as `0xRRGGBB`, or `None` for the theme's colour.
    pub fn color(self) -> Option<u32> {
        match self {
            Level::Neutral => None,
            Level::Normal => Some(0x22C55E),
            Level::Warning => Some(0xF59E0B),
            Level::Critical => Some(0xEF4444),
        }
    }
}

/// Level of `value`. Thresholds normally rise (`warning` below `critical`);
/// when `warning` is above `critical`, lower values are worse, as for free
/// space.
pub fn level(value: f64, warning: Option<f64>, critical: Option<f64>) -> Level {
    if warning.is_none() && critical.is_none() {
        return Level::Neutral;
    }
    let descending = matches!((warning, critical), (Some(w), Some(c)) if w > c);
    let reached = |threshold: Option<f64>| {
        threshold.is_some_and(|threshold| {
            if descending {
                value <= threshold
            } else {
                value >= threshold
            }
        })
    };
    if reached(critical) {
        Level::Critical
    } else if reached(warning) {
        Level::Warning
    } else {
        Level::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rounds_half_away_from_zero() {
        assert_eq!(format_fixed(2.5, 0), "3");
        assert_eq!(format_fixed(-2.5, 0), "-3");
        assert_eq!(format_fixed(9.96, 1), "10.0");
        assert_eq!(format_fixed(-0.04, 1), "0.0");
        assert_eq!(format_fixed(f64::NAN, 1), "0.0");
        assert_eq!(format_fixed(1.5, 40), "1.500000");
    }

    #[test]
    fn units_scale_and_suffix() {
        assert_eq!(format_value(0.4567, Unit::Fraction, Some(1)), "45.7%");
        assert_eq!(format_value(0.4567, Unit::Fraction, None), "45.7%");
        assert_eq!(format_value(99.0, Unit::Percent, None), "99%");
        assert_eq!(format_value(512.0, Unit::Bytes, None), "512 B");
        assert_eq!(format_value(1536.0, Unit::Bytes, None), "1.5 KiB");
        assert_eq!(
            format_value(5.5 * 1024.0 * 1024.0 * 1024.0, Unit::Bytes, Some(2)),
            "5.50 GiB"
        );
        assert_eq!(
            format_value(1_250_000.0, Unit::BytesPerSecond, None),
            "1.25 MB/s"
        );
        assert_eq!(format_value(999.0, Unit::BytesPerSecond, None), "999 B/s");
        assert_eq!(format_value(21.04, Unit::Celsius, Some(1)), "21.0 °C");
        assert_eq!(format_value(42.0, Unit::None, None), "42");
        assert_eq!(format_value(0.0, Unit::None, None), "0");
        assert_eq!(format_value(-0.0001, Unit::None, None), "0");
        assert_eq!(format_value(0.01234, Unit::None, None), "0.012");
        assert_eq!(format_value(f64::INFINITY, Unit::Bytes, None), "-");
    }

    #[test]
    fn durations_pick_a_readable_unit() {
        assert_eq!(format_value(0.25, Unit::Seconds, None), "250 ms");
        assert_eq!(format_value(0.0, Unit::Seconds, None), "0 s");
        assert_eq!(format_value(42.0, Unit::Seconds, None), "42 s");
        assert_eq!(format_value(90.0, Unit::Seconds, None), "1.5 min");
        assert_eq!(format_value(7_200.0, Unit::Seconds, None), "2 h");
        assert_eq!(
            format_value(3.5 * 86_400.0, Unit::Seconds, Some(1)),
            "3.5 d"
        );
        assert_eq!(format_value(2.0 * 31_536_000.0, Unit::Seconds, None), "2 y");
    }

    #[test]
    fn grafana_units_map_or_fall_back() {
        assert_eq!(Unit::from_grafana("percentunit"), Unit::Fraction);
        assert_eq!(Unit::from_grafana("Bps"), Unit::BytesPerSecond);
        assert_eq!(Unit::from_grafana("short"), Unit::None);
        assert_eq!(Unit::from_grafana("reqps"), Unit::None);
        assert_eq!(Unit::ALL[Unit::Celsius.index()], Unit::Celsius);
    }

    #[test]
    fn thresholds_colour_rising_and_falling_values() {
        assert_eq!(level(0.5, None, None), Level::Neutral);
        assert_eq!(level(0.5, Some(0.8), Some(0.9)), Level::Normal);
        assert_eq!(level(0.8, Some(0.8), Some(0.9)), Level::Warning);
        assert_eq!(level(0.95, Some(0.8), Some(0.9)), Level::Critical);
        assert_eq!(level(95.0, None, Some(90.0)), Level::Critical);
        assert_eq!(level(85.0, Some(80.0), None), Level::Warning);
        // Free space: lower is worse.
        assert_eq!(level(50.0, Some(20.0), Some(10.0)), Level::Normal);
        assert_eq!(level(15.0, Some(20.0), Some(10.0)), Level::Warning);
        assert_eq!(level(5.0, Some(20.0), Some(10.0)), Level::Critical);
        assert_eq!(Level::Neutral.color(), None);
        assert!(Level::Critical.color().is_some());
    }
}
