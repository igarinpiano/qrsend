//! A diagnostic log: what happened when, to send along with a report of a
//! problem (`--log FILE`, or the environment variable `QRSEND_LOG`).
//!
//! Like the web app's ("Copy log" there), it records no file names, no
//! contents, no device IDs and no network addresses: sizes, counts, kinds
//! and times only. Nothing is written unless a log was asked for.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

struct Log {
    out: Box<dyn Write + Send>,
    began: Instant,
    /// When a line that is written at intervals was last written.
    last: HashMap<&'static str, Instant>,
}

static LOG: OnceLock<Mutex<Log>> = OnceLock::new();

/// A date and time in UTC, from seconds since 1970.
fn utc(secs: u64) -> String {
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // (Days to a date: Howard Hinnant's `civil_from_days`.)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (
        doy - (153 * mp + 2) / 5 + 1,
        if mp < 10 { mp + 3 } else { mp - 9 },
    );
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

/// Starts the log if one was asked for (`path`, else `QRSEND_LOG`; "-" is
/// standard error). `command`: what is being run, without its arguments.
pub fn start(path: Option<&Path>, command: &str) -> Result<()> {
    let from_env = std::env::var_os("QRSEND_LOG").map(std::path::PathBuf::from);
    let Some(path) = path.or(from_env.as_deref()) else {
        return Ok(());
    };
    let out: Box<dyn Write + Send> = if path.as_os_str() == "-" {
        Box::new(std::io::stderr())
    } else {
        let file = File::create(path)
            .with_context(|| format!("cannot write the log to {}", path.display()))?;
        Box::new(BufWriter::new(file))
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut features = Vec::new();
    if cfg!(feature = "window") {
        features.push("window");
    }
    if cfg!(feature = "webrtc") {
        features.push("webrtc");
    }
    let mut log = Log {
        out,
        began: Instant::now(),
        last: HashMap::new(),
    };
    let _ = writeln!(
        log.out,
        "QRSend CLI {} · log of {} · qrsend {command}\nsystem: {} {} · {} threads · built with: {}\n\
         (No file names, contents, device IDs or network addresses are recorded.)\n",
        env!("CARGO_PKG_VERSION"),
        utc(now),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::thread::available_parallelism().map_or(0, |n| n.get()),
        if features.is_empty() {
            "no optional features".to_string()
        } else {
            features.join(", ")
        },
    );
    let _ = LOG.set(Mutex::new(log));
    Ok(())
}

/// Whether a log is being written.
pub fn on() -> bool {
    LOG.get().is_some()
}

fn write(log: &mut Log, area: &str, text: &str) {
    let at = log.began.elapsed().as_secs_f64();
    let _ = writeln!(log.out, "{at:8.2} {area:<6} {text}");
    let _ = log.out.flush();
}

/// One line: what part of the program says it, and what happened.
pub fn line(area: &str, text: impl FnOnce() -> String) {
    if let Some(log) = LOG.get() {
        write(&mut log.lock().unwrap(), area, &text());
    }
}

/// A line that says how things stand, written at most every `interval`.
pub fn every(key: &'static str, interval: Duration, area: &str, text: impl FnOnce() -> String) {
    let Some(log) = LOG.get() else { return };
    let mut log = log.lock().unwrap();
    let now = Instant::now();
    if log
        .last
        .get(key)
        .is_some_and(|at| now.duration_since(*at) < interval)
    {
        return;
    }
    log.last.insert(key, now);
    write(&mut log, area, &text());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc(1_791_540_576), "2026-10-09T10:09:36Z");
    }
}
