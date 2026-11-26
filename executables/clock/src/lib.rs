#![no_std]

#[cfg(any(test, target_arch = "wasm32"))]
extern crate std;
pub mod sound;
#[cfg(any(test, target_arch = "wasm32"))]
pub mod storage;

pub use time_zone::TimeZone;
use time_zone::{DateTime, LocalToUtc};

pub const MAX_LAPS: usize = 32;

/// The time zone of the guest: the POSIX TZ string of its `TZ` environment
/// variable, which Xila gives every task from the system setting. UTC when the
/// variable is missing or does not parse. The guest reads it once at start, so
/// a change in Settings applies when Clock starts again.
pub fn zone_from_environment(value: Option<&str>) -> TimeZone {
    value
        .and_then(|value| TimeZone::parse(value).ok())
        .unwrap_or_else(|| TimeZone::parse("UTC0").expect("UTC0 is a valid TZ string"))
}

/// The UTC instant of a local wall time, with the daylight saving rules of
/// alarms:
///
/// - A repeated local time (the autumn change) rings once, at its first
///   occurrence.
/// - A skipped local time (the spring change) rings after the change, as late
///   as the skipped hour: 02:30 becomes 03:30 when the clocks go from 02:00 to
///   03:00, as POSIX `mktime` does.
pub fn local_to_utc(zone: &TimeZone, local_seconds: i64) -> i64 {
    let utc_calendar = TimeZone::parse("UTC0").expect("UTC0 is a valid TZ string");
    let local = utc_calendar.from_utc(local_seconds).date_time;
    let date_time = DateTime {
        year: local.year,
        month: local.month,
        day: local.day,
        hour: local.hour,
        minute: local.minute,
        second: local.second,
    };
    match zone.to_utc(date_time) {
        LocalToUtc::Unique(utc) => utc,
        LocalToUtc::Ambiguous { earlier, .. } => earlier,
        // The offset before the change: transitions are more than a day apart.
        LocalToUtc::Nonexistent => {
            local_seconds - i64::from(zone.offset_at(local_seconds - 86_400))
        }
    }
}
pub const SETTINGS_SIZE: usize = 26;

#[derive(Default)]
pub struct AlarmAlert {
    ringing: bool,
    snooze_until: Option<u64>,
}

impl AlarmAlert {
    pub fn ring(&mut self) {
        self.ringing = true;
        self.snooze_until = None;
    }
    pub fn dismiss(&mut self) {
        self.ringing = false;
        self.snooze_until = None;
    }
    pub fn is_ringing(&self) -> bool {
        self.ringing
    }
    pub fn is_snoozed(&self) -> bool {
        self.snooze_until.is_some()
    }
    pub fn snooze(&mut self, now: u64) -> bool {
        if !self.ringing {
            return false;
        }
        let Some(deadline) = now.checked_add(5 * 60 * 1000) else {
            return false;
        };
        self.snooze_until = Some(deadline);
        self.ringing = false;
        true
    }
    pub fn tick(&mut self, now: u64) -> bool {
        if self.snooze_until.is_some_and(|deadline| now >= deadline) {
            self.ring();
            return true;
        }
        false
    }
}

/// Elapsed times are milliseconds from one monotonic epoch, never wall time.
#[derive(Default)]
pub struct Stopwatch {
    accumulated: u64,
    started: Option<u64>,
    laps: [u64; MAX_LAPS],
    lap_count: usize,
}

impl Stopwatch {
    pub fn elapsed(&self, now: u64) -> u64 {
        self.accumulated
            .saturating_add(self.started.map_or(0, |start| now.saturating_sub(start)))
    }
    pub fn start(&mut self, now: u64) {
        self.started.get_or_insert(now);
    }
    pub fn pause(&mut self, now: u64) {
        self.accumulated = self.elapsed(now);
        self.started = None;
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }
    pub fn lap(&mut self, now: u64) -> bool {
        if !self.is_running() || self.lap_count == MAX_LAPS {
            return false;
        }
        self.laps[self.lap_count] = self.elapsed(now);
        self.lap_count += 1;
        true
    }
    pub fn laps(&self) -> &[u64] {
        &self.laps[..self.lap_count]
    }
}

#[derive(Default)]
pub struct Timer {
    duration: u64,
    remaining: u64,
    deadline: Option<u64>,
    finished: bool,
}

impl Timer {
    pub fn start(&mut self, duration: u64, now: u64) -> bool {
        if duration == 0 || now.checked_add(duration).is_none() {
            return false;
        }
        self.duration = duration;
        self.remaining = duration;
        self.finished = false;
        self.deadline = Some(now + duration);
        true
    }
    pub fn remaining(&self, now: u64) -> u64 {
        self.deadline
            .map_or(self.remaining, |deadline| deadline.saturating_sub(now))
    }
    pub fn pause(&mut self, now: u64) {
        // Do not lose a completion if Pause is pressed after the deadline.
        if self.tick(now) {
            return;
        }
        self.remaining = self.remaining(now);
        self.deadline = None;
    }
    pub fn resume(&mut self, now: u64) {
        if self.deadline.is_none() && self.remaining != 0 {
            self.deadline = now.checked_add(self.remaining);
        }
    }
    pub fn tick(&mut self, now: u64) -> bool {
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            self.remaining = 0;
            self.finished = true;
            return true;
        }
        false
    }
    pub fn restart(&mut self, now: u64) {
        self.start(self.duration, now);
    }
    pub fn cancel(&mut self) {
        *self = Self::default();
    }
    pub fn is_running(&self) -> bool {
        self.deadline.is_some()
    }
    /// Length of the current countdown in milliseconds, zero when idle.
    pub fn duration(&self) -> u64 {
        self.duration
    }
    /// A countdown exists (running, paused or finished) until cancelled.
    pub fn is_active(&self) -> bool {
        self.duration != 0
    }
    pub fn finished(&self) -> bool {
        self.finished
    }
    pub fn acknowledge(&mut self) {
        self.finished = false;
    }
}

/// An alarm at a local wall time of the guest's time zone. Weekday bits are
/// Monday=1 through Sunday=64, in local days; zero means once. `next` and
/// `last_fired` are UTC seconds; see [`local_to_utc`] for the daylight saving
/// changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alarm {
    pub hour: u8,
    pub minute: u8,
    pub weekdays: u8,
    enabled: bool,
    next: Option<u64>,
    last_fired: Option<u64>,
}

impl Alarm {
    pub fn new(hour: u8, minute: u8, weekdays: u8) -> Option<Self> {
        if hour > 23 || minute > 59 || weekdays > 0x7f {
            return None;
        }
        Some(Self {
            hour,
            minute,
            weekdays,
            enabled: false,
            next: None,
            last_fired: None,
        })
    }
    pub fn arm(&mut self, now: u64, zone: &TimeZone) {
        self.enabled = true;
        self.schedule(now, zone);
    }
    /// Schedule again with `zone`, for example after Clock starts with a new
    /// time zone. An occurrence already due is kept, so that a missed alarm
    /// still rings once.
    pub fn reschedule(&mut self, now: u64, zone: &TimeZone) {
        if self.enabled && self.next.is_some_and(|next| next > now) {
            self.schedule(now, zone);
        }
    }
    fn schedule(&mut self, now: u64, zone: &TimeZone) {
        self.next = None;
        let after = now.max(self.last_fired.unwrap_or(0));
        let Ok(after_seconds) = i64::try_from(after) else {
            return;
        };
        let local_day =
            (after_seconds + i64::from(zone.offset_at(after_seconds))).div_euclid(86_400);
        // One day more than a week: the local day of `after` can be before or
        // after its UTC day.
        for offset in -1..=8 {
            let date = local_day + offset;
            let local = date * 86_400 + i64::from(self.hour) * 3600 + i64::from(self.minute) * 60;
            let candidate = local_to_utc(zone, local);
            let weekday = (date.rem_euclid(7) + 3) % 7;
            if candidate > after_seconds
                && (self.weekdays == 0 || self.weekdays & (1 << weekday) != 0)
            {
                self.next = u64::try_from(candidate).ok();
                break;
            }
        }
    }
    /// Coalesce missed occurrences into one alert and schedule strictly ahead.
    pub fn tick(&mut self, now: u64, zone: &TimeZone) -> bool {
        if !self.enabled || !self.next.is_some_and(|next| now >= next) {
            return false;
        }
        self.last_fired = Some(now);
        if self.weekdays == 0 {
            self.disable();
        } else {
            self.schedule(now, zone);
        }
        true
    }
    pub fn disable(&mut self) {
        self.enabled = false;
        self.next = None;
    }
    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn next(&self) -> Option<u64> {
        self.next
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub twelve_hour: bool,
    pub alarm: Option<Alarm>,
}

impl Settings {
    /// Fixed-size, versioned payload. Storage supplies the record checksum.
    pub fn encode(&self) -> [u8; SETTINGS_SIZE] {
        let mut bytes = [0; SETTINGS_SIZE];
        bytes[..4].copy_from_slice(b"CLK1");
        bytes[4] = u8::from(self.twelve_hour);
        if let Some(alarm) = self.alarm {
            bytes[5] = 1;
            bytes[6] = alarm.hour;
            bytes[7] = alarm.minute;
            bytes[8] = alarm.weekdays;
            bytes[9] = u8::from(alarm.enabled);
            bytes[10..18].copy_from_slice(&alarm.next.unwrap_or(u64::MAX).to_le_bytes());
            bytes[18..26].copy_from_slice(&alarm.last_fired.unwrap_or(u64::MAX).to_le_bytes());
        }
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SETTINGS_SIZE || &bytes[..4] != b"CLK1" || bytes[4] > 1 || bytes[5] > 1 {
            return None;
        }
        let mut settings = Self {
            twelve_hour: bytes[4] != 0,
            alarm: None,
        };
        if bytes[5] != 0 {
            let mut alarm = Alarm::new(bytes[6], bytes[7], bytes[8])?;
            if bytes[9] > 1 {
                return None;
            }
            alarm.enabled = bytes[9] != 0;
            let next = u64::from_le_bytes(bytes[10..18].try_into().ok()?);
            let last = u64::from_le_bytes(bytes[18..26].try_into().ok()?);
            alarm.next = (next != u64::MAX).then_some(next);
            alarm.last_fired = (last != u64::MAX).then_some(last);
            if alarm.enabled != alarm.next.is_some() {
                return None;
            }
            if alarm
                .next
                .zip(alarm.last_fired)
                .is_some_and(|(next, last)| next <= last)
            {
                return None;
            }
            settings.alarm = Some(alarm);
        }
        Some(settings)
    }
}

/// Longest timer the picker can select (23:59:59).
pub const MAX_TIMER_SECONDS: u32 = 24 * 3600 - 1;

/// Splits a timer duration into the picker's hour, minute and second rows.
pub const fn split_duration(seconds: u32) -> (u32, u32, u32) {
    let seconds = if seconds > MAX_TIMER_SECONDS {
        MAX_TIMER_SECONDS
    } else {
        seconds
    };
    (seconds / 3600, seconds / 60 % 60, seconds % 60)
}

/// Joins picker rows into a startable duration of at least one second.
pub const fn join_duration(hours: u32, minutes: u32, seconds: u32) -> u32 {
    let total = hours.saturating_mul(3600) + minutes.saturating_mul(60) + seconds;
    if total == 0 {
        1
    } else if total > MAX_TIMER_SECONDS {
        MAX_TIMER_SECONDS
    } else {
        total
    }
}

/// Remaining share of a countdown in thousandths, for the progress ring.
pub const fn remaining_permille(remaining: u64, total: u64) -> i32 {
    if total == 0 {
        return 0;
    }
    let remaining = if remaining > total { total } else { remaining };
    (remaining * 1000 / total) as i32
}

#[cfg(test)]
mod tests;
