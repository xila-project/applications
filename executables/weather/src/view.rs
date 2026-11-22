//! Presentation helpers: readable labels and chart series from API values.

use alloc::{format, string::String, vec::Vec};
use embedded_i18n::t;

/// Short description of a WMO weather interpretation code, as used by
/// Open-Meteo's `weather_code`.
pub const fn condition(code: u16) -> &'static str {
    match code {
        0 => t!("conditions.clear_sky"),
        1 => t!("conditions.mainly_clear"),
        2 => t!("conditions.partly_cloudy"),
        3 => t!("conditions.overcast"),
        45 | 48 => t!("conditions.fog"),
        51 | 53 | 55 => t!("conditions.drizzle"),
        56 | 57 => t!("conditions.freezing_drizzle"),
        61 => t!("conditions.light_rain"),
        63 => t!("conditions.rain"),
        65 => t!("conditions.heavy_rain"),
        66 | 67 => t!("conditions.freezing_rain"),
        71 => t!("conditions.light_snow"),
        73 => t!("conditions.snow"),
        75 => t!("conditions.heavy_snow"),
        77 => t!("conditions.snow_grains"),
        80..=82 => t!("conditions.rain_showers"),
        85 | 86 => t!("conditions.snow_showers"),
        95 => t!("conditions.thunderstorm"),
        96 | 99 => t!("conditions.thunderstorm_with_hail"),
        _ => t!("conditions.unknown"),
    }
}

/// Eight-point compass direction the wind comes from.
pub const fn compass(degrees: u16) -> &'static str {
    const POINTS: [&str; 8] = [
        t!("compass.north"),
        t!("compass.north_east"),
        t!("compass.east"),
        t!("compass.south_east"),
        t!("compass.south"),
        t!("compass.south_west"),
        t!("compass.west"),
        t!("compass.north_west"),
    ];
    // Each point covers 45 degrees centred on its heading.
    POINTS[(((degrees % 360) as u32 * 2 + 45) / 90 % 8) as usize]
}

/// Weekday of an ISO `YYYY-MM-DD` date (Gregorian calendar).
pub fn weekday(date: &str) -> Option<&'static str> {
    const NAMES: [&str; 7] = [
        t!("weekdays.sunday"),
        t!("weekdays.monday"),
        t!("weekdays.tuesday"),
        t!("weekdays.wednesday"),
        t!("weekdays.thursday"),
        t!("weekdays.friday"),
        t!("weekdays.saturday"),
    ];
    const OFFSETS: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut parts = date.get(..10)?.split('-');
    let mut year: i32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: i32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if month < 3 {
        year -= 1;
    }
    let index =
        (year + year / 4 - year / 100 + year / 400 + OFFSETS[month - 1] + day).rem_euclid(7);
    Some(NAMES[index as usize])
}

/// `Tue 10` from `2026-11-10`; the raw date if it cannot be parsed.
pub fn day_label(date: &str) -> String {
    match (weekday(date), date.get(8..10)) {
        (Some(name), Some(day)) => format!("{name} {}", day.trim_start_matches('0')),
        _ => String::from(date),
    }
}

/// `14:00` from `2026-11-10T14:00`.
pub fn hour_label(time: &str) -> &str {
    time.split_once('T').map_or(time, |(_, hour)| hour)
}

/// Chart points in tenths plus an axis range padded to whole units, so a
/// flat series still has visible headroom.
pub fn chart_series(values: &[f64]) -> (Vec<i32>, i32, i32) {
    let points: Vec<i32> = values
        .iter()
        .map(|value| (value * 10.0).round() as i32)
        .collect();
    let minimum = points.iter().copied().min().unwrap_or(0);
    let maximum = points.iter().copied().max().unwrap_or(0);
    // Round down/up to whole units, then add one unit on each side.
    let low = minimum.div_euclid(10) * 10 - 10;
    let high = (maximum + 9).div_euclid(10) * 10 + 10;
    (points, low, high)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_cover_common_codes() {
        assert_eq!(condition(0), "Clear sky");
        assert_eq!(condition(2), "Partly cloudy");
        assert_eq!(condition(63), "Rain");
        assert_eq!(condition(81), "Rain showers");
        assert_eq!(condition(99), "Thunderstorm, hail");
        assert_eq!(condition(1234), "Unknown");
    }

    #[test]
    fn compass_rounds_to_the_nearest_point() {
        assert_eq!(compass(0), "N");
        assert_eq!(compass(22), "N");
        assert_eq!(compass(23), "NE");
        assert_eq!(compass(230), "SW");
        assert_eq!(compass(338), "N");
        assert_eq!(compass(720), "N");
    }

    #[test]
    fn weekdays_and_labels() {
        assert_eq!(weekday("2026-11-10"), Some("Tue"));
        assert_eq!(weekday("2026-03-29"), Some("Sun"));
        assert_eq!(weekday("2000-02-29"), Some("Tue"));
        assert_eq!(weekday("2026-13-01"), None);
        assert_eq!(weekday("soon"), None);
        assert_eq!(day_label("2026-11-01"), "Sun 1");
        assert_eq!(day_label("bad"), "bad");
        assert_eq!(hour_label("2026-11-10T14:00"), "14:00");
        assert_eq!(hour_label("14:00"), "14:00");
    }

    #[test]
    fn chart_series_scales_and_pads_the_range() {
        let (points, low, high) = chart_series(&[11.94, 12.0, 18.25]);
        assert_eq!(points, [119, 120, 183]);
        assert_eq!((low, high), (100, 200));
        let (_, low, high) = chart_series(&[-2.5, -2.5]);
        assert_eq!((low, high), (-40, -10));
        let (points, low, high) = chart_series(&[]);
        assert!(points.is_empty());
        assert_eq!((low, high), (-10, 10));
    }
}
