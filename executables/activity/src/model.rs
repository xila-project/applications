// Window notifications can coalesce; queued actions must not wait for a slow
// metrics refresh if a notification races waker registration.
pub fn bounded_wait(
    deadline: core::time::Duration,
    now: core::time::Duration,
) -> core::time::Duration {
    deadline
        .saturating_sub(now)
        .min(core::time::Duration::from_millis(100))
}

use alloc::{format, string::String};

/// `1:05`, `2:03:04` or `3 d 04:05:06` from elapsed seconds.
pub fn format_uptime(seconds: u64) -> String {
    let (days, hours, minutes, seconds) = (
        seconds / 86_400,
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60,
    );
    match (days, hours) {
        (0, 0) => format!("{minutes}:{seconds:02}"),
        (0, _) => format!("{hours}:{minutes:02}:{seconds:02}"),
        _ => format!("{days} d {hours:02}:{minutes:02}:{seconds:02}"),
    }
}

/// Binary-prefixed size with at most one decimal, such as `763 KiB` or `1.5 MiB`.
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    // Tenths of the current unit, rounded to nearest.
    let mut tenths = (bytes as u128 * 10 + 512) / 1024;
    let mut unit = 0;
    while tenths >= 10_240 && unit + 1 < UNITS.len() {
        tenths = (tenths + 512) / 1024;
        unit += 1;
    }
    if tenths >= 1000 || tenths.is_multiple_of(10) {
        format!("{} {}", (tenths + 5) / 10, UNITS[unit])
    } else {
        format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[unit])
    }
}

/// Share of `capacity` in use, in thousandths, at most 1000.
pub fn permille(used: usize, capacity: usize) -> u32 {
    if capacity == 0 {
        return 0;
    }
    (used as u128 * 1000 / capacity as u128).min(1000) as u32
}

/// Chart points are stored in 16-byte steps so large heaps fit an `i32`.
pub fn chart_point(bytes: usize) -> i32 {
    (bytes / 16).min(i32::MAX as usize) as i32
}

/// Axis bounds around the visible samples, padded so that a flat series sits
/// mid-chart and small changes stay readable.
pub fn chart_range(samples: impl Iterator<Item = usize>) -> (usize, usize) {
    let (low, high) = samples.fold((usize::MAX, 0), |(low, high), sample| {
        (low.min(sample), high.max(sample))
    });
    if low > high {
        return (0, 4096);
    }
    let padding = ((high - low) / 4).max(2048);
    (low.saturating_sub(padding), high.saturating_add(padding))
}

#[derive(Debug)]
pub struct Config {
    pub interval_ms: u64,
}
impl Config {
    pub fn parse(args: &[String]) -> Result<Self, ()> {
        let interval_ms = match args {
            [] => 1000,
            [flag, value] if flag == "--interval-ms" => value.parse().map_err(|_| ())?,
            _ => return Err(()),
        };
        if !(100..=10000).contains(&interval_ms) {
            return Err(());
        }
        Ok(Self { interval_ms })
    }
}

pub struct MemorySample {
    pub used: usize,
    pub capacity: Option<usize>,
}
impl MemorySample {
    pub fn new(used: usize, capacity: usize) -> Self {
        Self {
            used,
            capacity: (capacity != 0 && used <= capacity).then_some(capacity),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec};
    #[test]
    fn refresh_configuration_is_bounded() {
        assert_eq!(Config::parse(&[]).unwrap().interval_ms, 1000);
        for valid in [100, 250, 10000] {
            let args = vec!["--interval-ms".to_string(), valid.to_string()];
            assert_eq!(Config::parse(&args).unwrap().interval_ms, valid);
        }
        for args in [
            vec!["--interval-ms"],
            vec!["--bad"],
            vec!["--interval-ms", "0"],
            vec!["--interval-ms", "99"],
            vec!["--interval-ms", "10001"],
            vec!["--interval-ms", "x"],
            vec!["--interval-ms", "100", "extra"],
        ] {
            assert!(
                Config::parse(
                    &args
                        .into_iter()
                        .map(ToString::to_string)
                        .collect::<alloc::vec::Vec<_>>()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn slow_sampling_still_services_queued_input_promptly() {
        use core::time::Duration;
        assert_eq!(
            bounded_wait(Duration::from_secs(10), Duration::ZERO),
            Duration::from_millis(100)
        );
        assert_eq!(
            bounded_wait(Duration::from_millis(25), Duration::ZERO),
            Duration::from_millis(25)
        );
        assert_eq!(
            bounded_wait(Duration::ZERO, Duration::from_secs(1)),
            Duration::ZERO
        );
    }

    #[test]
    fn readable_metric_values() {
        assert_eq!(format_uptime(7), "0:07");
        assert_eq!(format_uptime(105), "1:45");
        assert_eq!(format_uptime(3 * 3600 + 4 * 60 + 5), "3:04:05");
        assert_eq!(format_uptime(2 * 86_400 + 61), "2 d 00:01:01");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(4096), "4 KiB");
        assert_eq!(format_bytes(781_648), "763 KiB");
        assert_eq!(format_bytes(1024 * 1024 * 3 / 2), "1.5 MiB");
        assert_eq!(format_bytes(67_108_864), "64 MiB");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1 MiB");
        assert_eq!(permille(50, 200), 250);
        assert_eq!(permille(300, 200), 1000);
        assert_eq!(permille(1, 0), 0);
    }

    #[test]
    fn chart_range_pads_flat_and_growing_series() {
        assert_eq!(chart_range([].into_iter()), (0, 4096));
        assert_eq!(chart_range([10_000, 10_000].into_iter()), (7952, 12_048));
        assert_eq!(chart_range([1000].into_iter()), (0, 3048));
        assert_eq!(
            chart_range([100_000, 140_000].into_iter()),
            (90_000, 150_000)
        );
        assert_eq!(chart_point(4096), 256);
        assert_eq!(chart_point(usize::MAX), i32::MAX);
    }

    #[test]
    fn unavailable_capacity_is_explicit() {
        assert_eq!(MemorySample::new(50, 0).capacity, None);
        assert_eq!(MemorySample::new(50, 100).capacity, Some(100));
        assert_eq!(MemorySample::new(101, 100).capacity, None);
    }
}
