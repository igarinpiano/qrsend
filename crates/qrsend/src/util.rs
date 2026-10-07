use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.2} {}", UNITS[unit])
    }
}

pub fn human_duration(secs: f64) -> String {
    if !secs.is_finite() {
        return "?".into();
    }
    let s = secs.round() as u64;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{:02}s", s / 60, s % 60),
        _ => format!("{}h{:02}m", s / 3600, s % 3600 / 60),
    }
}

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn parse_session(s: &str) -> Result<u32> {
    u32::from_str_radix(s.trim().trim_start_matches("0x"), 16)
        .with_context(|| format!("invalid session id {s:?} (expected 8 hex digits)"))
}

/// Transfer speed over the last few seconds, from "bytes remaining" samples.
pub struct RateMeter {
    window: std::time::Duration,
    samples: std::collections::VecDeque<(std::time::Instant, u64)>,
}

impl RateMeter {
    pub fn new(window: std::time::Duration) -> Self {
        RateMeter {
            window,
            samples: Default::default(),
        }
    }

    /// Records the bytes still remaining now; returns bytes per second.
    pub fn update(&mut self, now: std::time::Instant, remaining: u64) -> f64 {
        self.samples.push_back((now, remaining));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > self.window {
            self.samples.pop_front();
        }
        let (t0, r0) = self.samples[0];
        let secs = now.duration_since(t0).as_secs_f64();
        if secs < 0.2 {
            0.0
        } else {
            r0.saturating_sub(remaining) as f64 / secs
        }
    }
}

/// Compact "0-5,7,9-12" rendering of sorted indices.
pub fn ranges(indices: &[u32]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < indices.len() {
        let start = indices[i];
        let mut end = start;
        while i + 1 < indices.len() && indices[i + 1] == end + 1 {
            i += 1;
            end += 1;
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    out.join(",")
}

pub fn parse_ranges(s: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    for part in s.split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => out.extend(a.parse::<u32>()?..=b.parse::<u32>()?),
            None => out.push(part.parse()?),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_roundtrip() {
        let v = vec![0, 1, 2, 5, 7, 8, 100];
        assert_eq!(ranges(&v), "0-2,5,7-8,100");
        assert_eq!(parse_ranges(&ranges(&v)).unwrap(), v);
        assert_eq!(parse_ranges("").unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn rate_meter_uses_a_recent_window() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let mut m = RateMeter::new(Duration::from_secs(5));
        assert_eq!(m.update(t0, 10_000), 0.0);
        assert_eq!(m.update(t0 + Duration::from_secs(1), 9_000), 1000.0);
        assert_eq!(m.update(t0 + Duration::from_secs(2), 8_000), 1000.0);
        // A long stall, then fast again: only the recent window counts.
        assert_eq!(m.update(t0 + Duration::from_secs(60), 8_000), 0.0);
        let r = m.update(t0 + Duration::from_secs(61), 6_000);
        assert!((r - 2000.0).abs() < 1.0, "{r}");
        // Remaining going up (a rejected segment) never yields a negative rate.
        assert_eq!(m.update(t0 + Duration::from_secs(62), 9_000), 0.0);
    }

    #[test]
    fn humanize() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.50 KiB");
        assert_eq!(human_duration(75.0), "1m15s");
        assert_eq!(human_duration(3725.0), "1h02m");
    }
}
