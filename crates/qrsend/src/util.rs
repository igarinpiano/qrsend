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
    fn humanize() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.50 KiB");
        assert_eq!(human_duration(75.0), "1m15s");
        assert_eq!(human_duration(3725.0), "1h02m");
    }
}
