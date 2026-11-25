use super::*;

fn utc() -> TimeZone {
    zone_from_environment(None)
}

const PARIS: &str = "CET-1CEST,M3.5.0,M10.5.0/3";

#[test]
fn stopwatch_uses_elapsed_time_and_excludes_pauses() {
    let mut watch = Stopwatch::default();
    watch.start(100);
    watch.start(200); // Starting twice must not lose elapsed time.
    assert_eq!(watch.elapsed(350), 250);
    watch.pause(350);
    assert_eq!(watch.elapsed(1000), 250);
    watch.start(1000);
    assert_eq!(watch.elapsed(1125), 375);
    watch.reset();
    assert_eq!(watch.elapsed(2000), 0);
    assert!(!watch.is_running());
}

#[test]
fn laps_are_bounded_and_capture_total_elapsed_time() {
    let mut watch = Stopwatch::default();
    assert!(!watch.lap(100));
    watch.start(100);
    for index in 0..MAX_LAPS {
        assert!(watch.lap(100 + index as u64 * 20));
    }
    assert!(!watch.lap(9999));
    assert_eq!(watch.laps()[MAX_LAPS - 1], (MAX_LAPS as u64 - 1) * 20);
    watch.reset();
    assert!(watch.laps().is_empty());
}

#[test]
fn timer_pauses_and_fires_once_even_after_a_delayed_poll() {
    let mut timer = Timer::default();
    assert!(!timer.start(0, 0));
    assert!(timer.start(5000, 100));
    timer.pause(1100);
    assert_eq!(timer.remaining(9000), 4000);
    timer.resume(9000);
    assert!(!timer.tick(12999));
    assert!(timer.tick(15000));
    assert!(!timer.tick(16000));
    assert_eq!(timer.remaining(16000), 0);
    assert!(timer.finished());
    timer.restart(20000);
    assert_eq!(timer.remaining(20000), 5000);
    timer.cancel();
    assert!(!timer.tick(30000));
    assert!(!timer.finished());
}

#[test]
fn one_time_alarm_rolls_to_tomorrow_and_survives_delayed_polling() {
    let mut alarm = Alarm::new(7, 30, 0).unwrap();
    alarm.arm(8 * 3600, &utc());
    assert_eq!(alarm.next(), Some(86400 + 7 * 3600 + 30 * 60));
    assert!(!alarm.tick(86400 + 7 * 3600, &utc()));
    assert!(alarm.tick(86400 + 8 * 3600, &utc()));
    assert!(!alarm.tick(86400 + 8 * 3600 + 1, &utc()));
    assert!(!alarm.enabled());
}

#[test]
fn recurring_alarm_does_not_ring_twice_after_a_backward_clock_change() {
    let mut alarm = Alarm::new(0, 1, 0x7f).unwrap();
    alarm.arm(0, &utc());
    assert!(alarm.tick(61, &utc()));
    assert!(!alarm.tick(50, &utc()));
    assert!(!alarm.tick(61, &utc()));
    assert!(alarm.tick(86461, &utc()));
}

#[test]
fn weekdays_use_monday_as_bit_zero_and_skip_the_weekend() {
    // 1970-01-01 is Thursday; day 1 is Friday, day 4 is Monday.
    let mut alarm = Alarm::new(8, 0, 0b000_0001).unwrap();
    alarm.arm(86400, &utc());
    assert_eq!(alarm.next(), Some(4 * 86400 + 8 * 3600));
    assert!(Alarm::new(24, 0, 0).is_none());
    assert!(Alarm::new(0, 60, 0).is_none());
    assert!(Alarm::new(0, 0, 0x80).is_none());
}

#[test]
fn settings_round_trip_rejects_truncation_and_invalid_values() {
    let mut settings = Settings {
        twelve_hour: true,
        alarm: Some(Alarm::new(23, 59, 0x1f).unwrap()),
    };
    settings.alarm.as_mut().unwrap().arm(0, &utc());
    let bytes = settings.encode();
    let decoded = Settings::decode(&bytes).unwrap();
    assert!(decoded.twelve_hour);
    assert_eq!(
        decoded.alarm.unwrap().next(),
        settings.alarm.unwrap().next()
    );
    assert!(Settings::decode(&bytes[..bytes.len() - 1]).is_none());
    let mut bad = bytes;
    bad[0] = 0;
    assert!(Settings::decode(&bad).is_none());
}

#[test]
fn snooze_uses_elapsed_time_and_dismiss_cancels_a_pending_alert() {
    let mut alert = AlarmAlert::default();
    assert!(!alert.snooze(0));
    alert.ring();
    assert!(alert.is_ringing());
    assert!(alert.snooze(1000));
    assert!(!alert.is_ringing());
    assert!(!alert.tick(300_999));
    assert!(alert.tick(301_000));
    assert!(!alert.tick(301_001));
    assert!(alert.snooze(302_000));
    alert.dismiss();
    assert!(!alert.tick(999_999));
    assert!(!alert.is_ringing());
}

#[test]
fn timer_picker_rows_round_trip_and_clamp() {
    assert_eq!(split_duration(300), (0, 5, 0));
    assert_eq!(split_duration(3725), (1, 2, 5));
    assert_eq!(split_duration(u32::MAX), (23, 59, 59));
    assert_eq!(join_duration(1, 2, 5), 3725);
    // A zero selection still starts a one-second countdown.
    assert_eq!(join_duration(0, 0, 0), 1);
    assert_eq!(join_duration(99, 99, 99), MAX_TIMER_SECONDS);
    for seconds in [1, 59, 60, 3599, 3600, MAX_TIMER_SECONDS] {
        let (h, m, s) = split_duration(seconds);
        assert_eq!(join_duration(h, m, s), seconds);
    }
}

#[test]
fn progress_ring_tracks_the_remaining_share() {
    assert_eq!(remaining_permille(30_000, 60_000), 500);
    assert_eq!(remaining_permille(60_000, 60_000), 1000);
    assert_eq!(remaining_permille(90_000, 60_000), 1000);
    assert_eq!(remaining_permille(0, 60_000), 0);
    assert_eq!(remaining_permille(5, 0), 0);
}

#[test]
fn timer_stays_active_until_cancelled() {
    let mut timer = Timer::default();
    assert!(!timer.is_active());
    timer.start(1000, 0);
    assert!(timer.is_active());
    assert_eq!(timer.duration(), 1000);
    timer.pause(400);
    assert!(timer.is_active() && !timer.is_running());
    timer.resume(400);
    assert!(timer.tick(1400));
    assert!(timer.is_active() && timer.finished());
    timer.cancel();
    assert!(!timer.is_active());
    assert_eq!(timer.duration(), 0);
}

#[test]
fn the_zone_comes_from_tz() {
    assert_eq!(zone_from_environment(None).offset_at(0), 0);
    assert_eq!(zone_from_environment(Some("not a zone")).offset_at(0), 0);
    assert_eq!(zone_from_environment(Some("JST-9")).offset_at(0), 9 * 3600);
}

#[test]
fn alarms_ring_at_local_time() {
    let paris = zone_from_environment(Some(PARIS));
    // 2026-07-15 12:00 UTC is 14:00 in Paris: 07:00 tomorrow is 05:00 UTC.
    let mut alarm = Alarm::new(7, 0, 0).unwrap();
    alarm.arm(1_784_116_800, &paris);
    assert_eq!(alarm.next(), Some(1_784_178_000));
    // Weekdays are local days: in UTC+14, Sunday 2026-11-22 12:00 UTC is
    // already Monday 02:00, so a Monday 08:00 alarm is later the same UTC day.
    let kiribati = zone_from_environment(Some("<+14>-14"));
    let mut monday = Alarm::new(8, 0, 0b000_0001).unwrap();
    monday.arm(1_795_348_800, &kiribati);
    assert_eq!(monday.next(), Some(1_795_370_400));
}

#[test]
fn a_skipped_alarm_time_rings_after_the_spring_change() {
    let paris = zone_from_environment(Some(PARIS));
    let mut alarm = Alarm::new(2, 30, 0x7f).unwrap();
    // 2026-03-28 13:00 in Paris. On 2026-03-29, 02:00 CET becomes 03:00 CEST,
    // so 02:30 does not exist: the alarm rings at 03:30 CEST (01:30 UTC).
    alarm.arm(1_774_699_200, &paris);
    assert_eq!(alarm.next(), Some(1_774_747_800));
    assert!(alarm.tick(1_774_747_800, &paris));
    // The next day, 02:30 CEST is 00:30 UTC.
    assert_eq!(alarm.next(), Some(1_774_830_600));
}

#[test]
fn a_repeated_alarm_time_rings_once_in_autumn() {
    let paris = zone_from_environment(Some(PARIS));
    let mut alarm = Alarm::new(2, 30, 0x7f).unwrap();
    // On 2026-10-25, 03:00 CEST becomes 02:00 CET: 02:30 happens at 00:30 UTC
    // and again at 01:30 UTC. The alarm rings at the first one only.
    alarm.arm(1_792_843_200, &paris);
    assert_eq!(alarm.next(), Some(1_792_888_200));
    assert!(alarm.tick(1_792_888_200, &paris));
    assert!(!alarm.tick(1_792_891_800, &paris));
    // The next day, 02:30 CET is 01:30 UTC.
    assert_eq!(alarm.next(), Some(1_792_978_200));
}

#[test]
fn a_new_time_zone_reschedules_future_alarms_only() {
    let paris = zone_from_environment(Some(PARIS));
    let mut alarm = Alarm::new(7, 0, 0).unwrap();
    alarm.arm(1_784_116_800, &utc());
    assert_eq!(alarm.next(), Some(1_784_178_000 + 2 * 3600));
    alarm.reschedule(1_784_116_800, &paris);
    assert_eq!(alarm.next(), Some(1_784_178_000));
    // An overdue occurrence is kept, so that it still rings once.
    alarm.reschedule(1_784_178_001, &utc());
    assert_eq!(alarm.next(), Some(1_784_178_000));
    assert!(alarm.tick(1_784_178_001, &paris));
}
