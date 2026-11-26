//! Civil (proleptic Gregorian) date and time conversion for the shell's time
//! commands, plus a small ISO 8601 UTC parser and formatter. The conversions
//! are the classic `days_from_civil`/`civil_from_days` pair, valid over the
//! whole signed 64-bit second range.

use alloc::string::String;

/// Nanoseconds per second.
pub(super) const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// Seconds per day.
const SECONDS_PER_DAY: i64 = 86_400;

/// A broken-down UTC time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CivilTime {
    pub year: i64,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanoseconds: u32,
}

/// Days from 1970-01-01 to the given proleptic Gregorian date.
pub(super) fn days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`]: the Gregorian date `days` after
/// 1970-01-01.
pub(super) fn civil_from_days(days: i64) -> (i64, u8, u8) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u8;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u8;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Breaks signed Unix seconds and nanoseconds down into a [`CivilTime`].
pub(super) fn civil_from_unix(seconds: i64, nanoseconds: u32) -> CivilTime {
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    CivilTime {
        year,
        month,
        day,
        hour: (second_of_day / 3_600) as u8,
        minute: ((second_of_day % 3_600) / 60) as u8,
        second: (second_of_day % 60) as u8,
        nanoseconds,
    }
}

/// Formats a [`CivilTime`] as `YYYY-MM-DD HH:MM:SS`, with a nine-digit
/// fractional part when `nanoseconds` is nonzero.
pub(super) fn format_civil(time: CivilTime) -> String {
    use alloc::format;
    if time.nanoseconds == 0 {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            time.year, time.month, time.day, time.hour, time.minute, time.second
        )
    } else {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:09}",
            time.year, time.month, time.day, time.hour, time.minute, time.second, time.nanoseconds
        )
    }
}

/// Whether `year` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

/// Days in `month` of `year`.
fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Parses exactly four ASCII digits.
fn parse_four_digits(text: &[u8]) -> Option<i64> {
    if text.len() != 4 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        i64::from(text[0] - b'0') * 1_000
            + i64::from(text[1] - b'0') * 100
            + i64::from(text[2] - b'0') * 10
            + i64::from(text[3] - b'0'),
    )
}

/// Parses exactly two ASCII digits.
fn parse_two_digits(text: &[u8]) -> Option<u8> {
    if text.len() != 2 || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some((text[0] - b'0') * 10 + (text[1] - b'0'))
}

/// Parses an ISO 8601 UTC time: `YYYY-MM-DDTHH:MM:SS`, an optional fractional
/// part of one to nine digits, and a `Z` or `+00:00`/`-00:00` UTC designator.
///
/// Returns the signed Unix seconds and the nanoseconds within the second.
/// Any other offset than zero is rejected: this command sets UTC only.
pub(super) fn parse_iso8601_utc(text: &str) -> Option<(i64, u32)> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year = parse_four_digits(&bytes[0..4])?;
    if bytes[4] != b'-' {
        return None;
    }
    let month = parse_two_digits(&bytes[5..7])?;
    if bytes[7] != b'-' {
        return None;
    }
    let day = parse_two_digits(&bytes[8..10])?;
    if bytes[10] != b'T' {
        return None;
    }
    let hour = parse_two_digits(&bytes[11..13])?;
    if bytes[13] != b':' {
        return None;
    }
    let minute = parse_two_digits(&bytes[14..16])?;
    if bytes[16] != b':' {
        return None;
    }
    let second = parse_two_digits(&bytes[17..19])?;
    if month == 0 || day == 0 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }

    let mut index = 19;
    let mut nanoseconds = 0u32;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let digits = index - start;
        if digits == 0 || digits > 9 {
            return None;
        }
        let mut scale = 100_000_000u32;
        for digit in &bytes[start..index] {
            nanoseconds += u32::from(digit - b'0') * scale;
            scale /= 10;
        }
    }

    let rest = &bytes[index..];
    if rest == b"Z" || rest == b"+00:00" || rest == b"-00:00" {
        // UTC designator accepted.
    } else {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(SECONDS_PER_DAY)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    Some((seconds, nanoseconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for seconds in [
            0,
            1,
            -1,
            86_399,
            86_400,
            -86_400,
            1_800_000_000,
            -2_208_988_800, // 1900-01-01T00:00:00Z
            951_782_400,    // 2000-02-29T00:00:00Z
        ] {
            let civil = civil_from_unix(seconds, 0);
            assert_eq!(
                days_from_civil(civil.year, civil.month, civil.day) * SECONDS_PER_DAY
                    + i64::from(civil.hour) * 3_600
                    + i64::from(civil.minute) * 60
                    + i64::from(civil.second),
                seconds,
                "{seconds}"
            );
        }
        assert_eq!(
            civil_from_unix(1_800_000_000, 0),
            CivilTime {
                year: 2027,
                month: 1,
                day: 15,
                hour: 8,
                minute: 0,
                second: 0,
                nanoseconds: 0,
            }
        );
        assert_eq!(
            civil_from_unix(-1, 0),
            CivilTime {
                year: 1969,
                month: 12,
                day: 31,
                hour: 23,
                minute: 59,
                second: 59,
                nanoseconds: 0,
            }
        );
    }

    #[test]
    fn parses_iso8601_utc() {
        assert_eq!(parse_iso8601_utc("1970-01-01T00:00:00Z"), Some((0, 0)));
        assert_eq!(parse_iso8601_utc("1969-12-31T23:59:59Z"), Some((-1, 0)));
        assert_eq!(
            parse_iso8601_utc("2027-01-15T08:00:00Z"),
            Some((1_800_000_000, 0))
        );
        assert_eq!(
            parse_iso8601_utc("2027-01-15T08:00:00.5Z"),
            Some((1_800_000_000, 500_000_000))
        );
        assert_eq!(
            parse_iso8601_utc("2027-01-15T08:00:00.123456789+00:00"),
            Some((1_800_000_000, 123_456_789))
        );
        assert_eq!(
            parse_iso8601_utc("2000-02-29T12:00:00-00:00"),
            Some((951_825_600, 0))
        );
    }

    #[test]
    fn rejects_invalid_iso8601() {
        for text in [
            "",
            "2027-01-15T08:00:00",
            "2027-01-15 08:00:00Z",
            "2027-01-15T08:00:00+01:00",
            "2027-01-15T08:00:00z",
            "2027-13-15T08:00:00Z",
            "2027-02-30T08:00:00Z",
            "2023-02-29T00:00:00Z",
            "2027-01-15T24:00:00Z",
            "2027-01-15T08:60:00Z",
            "2027-01-15T08:00:60Z",
            "2027-01-15T08:00:00.Z",
            "2027-01-15T08:00:00.1234567890Z",
            "2027-1-15T08:00:00Z",
            "202X-01-15T08:00:00Z",
        ] {
            assert_eq!(parse_iso8601_utc(text), None, "{text}");
        }
    }

    #[test]
    fn formats_civil_time() {
        assert_eq!(
            format_civil(civil_from_unix(1_800_000_000, 0)),
            "2027-01-15 08:00:00"
        );
        assert_eq!(
            format_civil(civil_from_unix(1_800_000_000, 5)),
            "2027-01-15 08:00:00.000000005"
        );
    }
}
