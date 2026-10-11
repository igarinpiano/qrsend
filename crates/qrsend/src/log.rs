//! A diagnostic log: what happened when, to send along with a report of a
//! problem.
//!
//! Every `send` and `recv` keeps one without being asked, so that it is there
//! after something went wrong: in the cache (`logs/`), the last [`KEPT`] runs,
//! each of a bounded length. `qrsend log` prints the latest. `--log FILE` (or
//! the environment variable `QRSEND_LOG`) writes it elsewhere instead — all of
//! it, however long the run: only the logs kept here are bounded. "-" is
//! standard error, and `QRSEND_LOG=off` keeps none at all.
//!
//! Like the web app's ("Copy log" there), it records no file names, no
//! contents, no device IDs and no network addresses: sizes, counts, kinds
//! and times only. The text of an error from elsewhere (a library, the
//! system) goes through [`discreet`] first, which takes addresses out.
//!
//! A log is shown on terminals and pasted into reports, so nothing in it may
//! be more than text: every line is written with what a terminal would act
//! on escaped, and what `qrsend log` shows is escaped again (the file could
//! be anyone's, were its directory not checked to be this user's alone).

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Subcommand;

use crate::{extract, paths, util};

/// How many runs' logs are kept.
pub const KEPT: usize = 10;
/// The most lines a kept log holds: beyond that, the middle is left out.
const MAX_LINES: usize = 2000;
/// Kept whatever follows: how the run began.
const KEEP_FIRST: usize = 60;
/// The most characters of one line (the longest the program writes are a
/// third of this: whatever is longer was not meant to be).
const MAX_LINE: usize = 600;
/// The most that is read of a kept log to show it (several times what one
/// can hold).
const MAX_SHOWN: u64 = 4 << 20;

enum Out {
    /// Where the user said: every line, as it comes, and without a bound
    /// (the user chose the place, and looks after it).
    Given(Box<dyn Write + Send>),
    Kept(Kept),
}

/// A log in the cache. It is on disk line by line (a run that is killed
/// leaves what it had written), and never grows beyond [`MAX_LINES`].
struct Kept {
    path: PathBuf,
    file: Option<File>,
    header: String,
    first: Vec<String>,
    rest: VecDeque<String>,
    dropped: usize,
}

impl Kept {
    /// A new log for this run; older ones beyond [`KEPT`] are removed.
    fn create(command: &str, now: u64, header: String) -> Option<Kept> {
        let dir = paths::logs_dir();
        // (Like everything in the cache: for this user only.)
        paths::create_private(&dir).ok()?;
        let stamp = utc(now).replace(['-', ':'], "");
        let path = dir.join(format!("{stamp}-{command}-{}.log", std::process::id()));
        paths::write_private(&path, header.as_bytes()).ok()?;
        let file = OpenOptions::new().append(true).open(&path).ok()?;
        for old in kept().into_iter().skip(KEPT) {
            let _ = fs::remove_file(old.path);
        }
        Some(Kept {
            path,
            file: Some(file),
            header,
            first: Vec::new(),
            rest: VecDeque::new(),
            dropped: 0,
        })
    }

    fn push(&mut self, line: String) {
        if let Some(file) = &mut self.file {
            let _ = file.write_all(format!("{line}\n").as_bytes());
        }
        if self.first.len() < KEEP_FIRST {
            self.first.push(line);
            return;
        }
        self.rest.push_back(line);
        if self.first.len() + self.rest.len() > MAX_LINES {
            // A stretch at once, so that the file is seldom written anew.
            let cut = MAX_LINES / 4;
            self.rest.drain(..cut);
            self.dropped += cut;
            self.rewrite();
        }
    }

    /// Writes the file anew, without the lines left out.
    fn rewrite(&mut self) {
        self.file = None;
        let mut text = self.header.clone();
        for line in &self.first {
            text += &format!("{line}\n");
        }
        text += &format!("         … {} lines left out …\n", self.dropped);
        for line in &self.rest {
            text += &format!("{line}\n");
        }
        self.file = paths::write_private(&self.path, text.as_bytes())
            .and_then(|()| OpenOptions::new().append(true).open(&self.path))
            .ok();
    }
}

struct Log {
    out: Out,
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

/// Starts the log of this run. `path` (else `QRSEND_LOG`) says where to write
/// it: a file, "-" for standard error; without either it is kept in the cache
/// (see the module's description). `command`: what is being run, without its
/// arguments.
pub fn start(path: Option<&Path>, command: &str) -> Result<()> {
    let from_env = std::env::var_os("QRSEND_LOG")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    if path.is_none() && from_env.as_deref().is_some_and(|p| p.as_os_str() == "off") {
        return Ok(());
    }
    let now = util::now().max(0) as u64;
    let mut features = Vec::new();
    if cfg!(feature = "window") {
        features.push("window");
    }
    if cfg!(feature = "webrtc") {
        features.push("webrtc");
    }
    let header = format!(
        "QRSend CLI {} · log of {} · qrsend {command}\nsystem: {} {} · {} threads · built with: {}\n\
         (No file names, contents, device IDs or network addresses are recorded.)\n\n",
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
    let out = match path.or(from_env.as_deref()) {
        Some(path) => {
            let mut out: Box<dyn Write + Send> = if path.as_os_str() == "-" {
                Box::new(std::io::stderr())
            } else {
                let file = File::create(path)
                    .with_context(|| format!("cannot write the log to {}", path.display()))?;
                Box::new(BufWriter::new(file))
            };
            let _ = out.write_all(header.as_bytes());
            Out::Given(out)
        }
        // (A log that cannot be kept is no reason not to transfer.)
        None => match Kept::create(command, now, header) {
            Some(kept) => Out::Kept(kept),
            None => return Ok(()),
        },
    };
    let _ = LOG.set(Mutex::new(Log {
        out,
        began: Instant::now(),
        last: HashMap::new(),
    }));
    Ok(())
}

/// Whether a log is being written.
pub fn on() -> bool {
    LOG.get().is_some()
}

/// Whether this run's log is one of those kept (which `qrsend log` prints).
pub fn kept_this_run() -> bool {
    LOG.get()
        .is_some_and(|log| matches!(log.lock().unwrap().out, Out::Kept(_)))
}

impl Out {
    fn push(&mut self, line: String) {
        match self {
            Out::Given(out) => {
                let _ = writeln!(out, "{line}");
                let _ = out.flush();
            }
            Out::Kept(kept) => kept.push(line),
        }
    }
}

fn write(log: &mut Log, area: &str, text: &str) {
    let at = log.began.elapsed().as_secs_f64();
    log.out
        .push(format!("{at:8.2} {area:<6} {}", one_line(text)));
}

/// A text as one line of the log: what a terminal would act on is escaped (a
/// line break too, so that nothing passes for a line of its own), and it is
/// cut where no line the program means to write ends.
fn one_line(text: &str) -> String {
    let mut line = util::printable(text);
    if let Some((at, _)) = line.char_indices().nth(MAX_LINE) {
        line.truncate(at);
        line.push('…');
    }
    line
}

/// The text of an error that is not this program's own (a library's, the
/// system's), fit for the log: what went wrong, without whom it names. Every
/// word that holds a network address (with a port or without, in brackets,
/// in a longer expression) or a name on the local network is left out.
pub fn discreet(text: &str) -> String {
    let v4 = |word: &str| {
        word.split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .any(|part| part.trim_end_matches('.').parse::<Ipv4Addr>().is_ok())
    };
    let v6 = |word: &str| {
        word.split(|c: char| !(c.is_ascii_hexdigit() || c == ':'))
            .filter(|part| part.matches(':').count() >= 2)
            .any(|part| {
                part.parse::<Ipv6Addr>().is_ok()
                    || part
                        .strip_suffix(':')
                        .is_some_and(|p| p.parse::<Ipv6Addr>().is_ok())
            })
    };
    let name = |word: &str| word.to_ascii_lowercase().contains(".local");
    text.split(' ')
        .map(|word| match v4(word) || v6(word) || name(word) {
            true => "<address>",
            false => word,
        })
        .collect::<Vec<_>>()
        .join(" ")
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

/// The last line of a run: how it ended. An error's text is left out, since
/// it may name a file.
pub fn end(ok: bool) {
    line("end", || {
        if ok {
            "finished".into()
        } else {
            "stopped with an error (its text is on the terminal only: it may name files)".into()
        }
    });
}

/// A log kept from an earlier run.
pub struct KeptLog {
    pub path: PathBuf,
    /// When the run began (UTC), as in the file's name.
    pub began: String,
    /// `send` or `recv`.
    pub command: String,
}

/// The logs kept, latest first.
pub fn kept() -> Vec<KeptLog> {
    let mut out: Vec<KeptLog> = fs::read_dir(paths::logs_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            // (A file, not a link to one: only what this program wrote.)
            if path.extension()? != "log" || !fs::symlink_metadata(&path).ok()?.is_file() {
                return None;
            }
            // 20261009T100936Z-recv-1234
            let name = path.file_stem()?.to_str()?.to_string();
            let mut parts = name.splitn(3, '-');
            let (stamp, command) = (parts.next()?, parts.next()?);
            let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
            if stamp.len() != 16 || !plain(stamp) || !plain(command) {
                return None;
            }
            let s = |range: std::ops::Range<usize>| stamp.get(range);
            Some(KeptLog {
                began: format!(
                    "{}-{}-{} {}:{}:{} UTC",
                    s(0..4)?,
                    s(4..6)?,
                    s(6..8)?,
                    s(9..11)?,
                    s(11..13)?,
                    s(13..15)?
                ),
                command: command.to_string(),
                path,
            })
        })
        .collect();
    // By the second each began (the name); within a second, by which was
    // written to last.
    let written = |log: &KeptLog| fs::metadata(&log.path).and_then(|m| m.modified()).ok();
    out.sort_by_cached_key(|log| std::cmp::Reverse((log.began.clone(), written(log))));
    out
}

/// The logs kept, for showing them: only from a directory that is this
/// user's alone (see [`paths::check_private`]).
fn kept_here() -> Result<Vec<KeptLog>> {
    let dir = paths::logs_dir();
    if fs::symlink_metadata(&dir).is_ok() {
        paths::check_private(&dir).context("the logs are not shown")?;
    }
    Ok(kept())
}

/// Removes the logs kept; returns how many there were.
pub fn clear() -> usize {
    let n = kept().len();
    // (The directory: also what an interrupted rewrite may have left.)
    let _ = fs::remove_dir_all(paths::logs_dir());
    n
}

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct LogArgs {
    #[command(subcommand)]
    cmd: Option<LogCmd>,
    #[command(flatten)]
    show: ShowArgs,
}

/// How many runs before the latest one, written the way git writes "how many
/// commits before HEAD": `HEAD` (or `@`) is the latest run, `HEAD^` and
/// `HEAD~1` the one before it, `HEAD~3` three before it. The signs add up
/// (`HEAD^^`, `HEAD~2^`), and may stand alone (`^`, `~3`).
fn parse_back(s: &str) -> Result<usize, String> {
    let bad = || {
        format!(
            "invalid run {s:?} (HEAD is the latest, HEAD^ or HEAD~1 the one before it, HEAD~2 the one before that…)"
        )
    };
    let mut rest = match s.get(..4) {
        Some(head) if head.eq_ignore_ascii_case("HEAD") => &s[4..],
        _ => s.strip_prefix('@').unwrap_or(s),
    };
    if s.is_empty() {
        return Err(bad());
    }
    let mut back = 0usize;
    while !rest.is_empty() {
        let step = if let Some(after) = rest.strip_prefix('^') {
            rest = after;
            1
        } else if let Some(after) = rest.strip_prefix('~') {
            let digits = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            rest = &after[digits..];
            match digits {
                0 => 1,
                _ => after[..digits].parse().map_err(|_| bad())?,
            }
        } else {
            return Err(bad());
        };
        back = back.checked_add(step).ok_or_else(bad)?;
    }
    Ok(back)
}

#[derive(clap::Args)]
pub struct ShowArgs {
    /// Which run, counted back from the latest the way git counts commits:
    /// HEAD is the latest, HEAD^ or HEAD~1 the one before it, HEAD~2 the one
    /// before that… (`qrsend log list` shows what to write for each. Some
    /// shells take a ^ for themselves: HEAD~1 is safe in all of them)
    #[arg(default_value = "HEAD", value_name = "RUN", value_parser = parse_back)]
    which: usize,
    /// Copy the log to the clipboard instead of printing it
    #[arg(long)]
    copy: bool,
}

#[derive(Subcommand)]
pub enum LogCmd {
    /// Print a log (what `qrsend log` alone does)
    Show(ShowArgs),
    /// List the logs kept
    List,
    /// Remove the logs kept
    Clear,
}

/// What to write for the log `back` runs before the latest.
fn name_back(back: usize) -> String {
    match back {
        0 => "HEAD".into(),
        n => format!("HEAD~{n}"),
    }
}

/// `qrsend log`: the logs of the last runs.
pub fn command(args: LogArgs) -> Result<()> {
    match args.cmd.unwrap_or(LogCmd::Show(args.show)) {
        LogCmd::Show(show) => {
            let logs = kept_here()?;
            if logs.is_empty() {
                eprintln!(
                    "No log is kept. `qrsend send` and `qrsend recv` keep one each time they run{}.",
                    if std::env::var_os("QRSEND_LOG").is_some() {
                        " — unless QRSEND_LOG says otherwise, as it does now"
                    } else {
                        ""
                    }
                );
                return Ok(());
            }
            let Some(log) = logs.get(show.which) else {
                bail!(
                    "there are only {} logs: the oldest is {} (see `qrsend log list`)",
                    logs.len(),
                    name_back(logs.len() - 1)
                );
            };
            let mut text = Vec::new();
            File::open(&log.path)
                .and_then(|file| file.take(MAX_SHOWN).read_to_end(&mut text))
                .with_context(|| format!("cannot read {}", log.path.display()))?;
            // (Text and nothing else, whatever the file holds: it goes to a
            // terminal, or into a report.)
            let text = util::printable_text(&String::from_utf8_lossy(&text));
            if show.copy {
                extract::copy_to_clipboard(&text)?;
                eprintln!(
                    "Copied the log of `qrsend {}` from {} to the clipboard.",
                    log.command, log.began
                );
            } else {
                print!("{text}");
            }
        }
        LogCmd::List => {
            let logs = kept_here()?;
            if logs.is_empty() {
                println!("No log is kept.");
                return Ok(());
            }
            let now = util::now();
            for (i, log) in logs.iter().enumerate() {
                let (size, modified) = util::disk_use(&log.path);
                println!(
                    "{:<7}  {}  {:<4}  {:>10}  {}",
                    name_back(i),
                    log.began,
                    log.command,
                    util::human_bytes(size),
                    util::human_age(now - modified)
                );
            }
            println!(
                "\nIn {} (the last {KEPT} runs are kept).\n\
                 `qrsend log` prints the latest, `qrsend log HEAD~1` the one before it;\n\
                 `qrsend log clear` removes them.",
                paths::logs_dir().display()
            );
        }
        LogCmd::Clear => match clear() {
            0 => println!("No log is kept."),
            n => println!("Removed {n} log(s)."),
        },
    }
    Ok(())
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

    #[test]
    fn runs_are_counted_back_as_git_counts_commits() {
        for (written, back) in [
            ("HEAD", 0),
            ("@", 0),
            ("head", 0),
            ("HEAD^", 1),
            ("HEAD~", 1),
            ("HEAD~1", 1),
            ("@^", 1),
            ("^", 1),
            ("HEAD^^", 2),
            ("HEAD~2", 2),
            ("~2", 2),
            ("HEAD~2^", 3),
            ("@~10", 10),
            ("HEAD~0", 0),
        ] {
            assert_eq!(parse_back(written), Ok(back), "{written}");
        }
        for written in [
            "",
            "2",
            "HEAD2",
            "HEAD~x",
            "latest",
            "HEAD~99999999999999999999999",
        ] {
            assert!(parse_back(written).is_err(), "{written}");
        }
        assert_eq!(name_back(0), "HEAD");
        assert_eq!(parse_back(&name_back(7)), Ok(7));
    }

    /// A log goes to terminals and into reports: whatever a line is made
    /// from, it is one line of text, and not an endless one.
    #[test]
    fn a_line_is_one_line_of_text() {
        assert_eq!(one_line("codes=12 color=false"), "codes=12 color=false");
        assert_eq!(
            one_line("failed: \u{1b}]0;owned\u{7}\n    0.00 end    finished"),
            "failed: \\u{1b}]0;owned\\u{7}\\u{a}    0.00 end    finished"
        );
        assert_eq!(one_line("a\u{202e}b\rc"), "a\\u{202e}b\\u{d}c");
        let long = one_line(&"é".repeat(5 * MAX_LINE));
        assert_eq!(long.chars().count(), MAX_LINE + 1);
        assert!(long.ends_with('…'));
    }

    /// The log names no one: an error's text from a library or the system
    /// is written without the addresses in it.
    #[test]
    fn errors_are_logged_without_addresses() {
        for (said, logged) in [
            ("connection refused", "connection refused"),
            (
                "cannot reach 192.168.1.20:50000 (os error 61)",
                "cannot reach <address> (os error 61)",
            ),
            ("no route to 10.0.0.7.", "no route to <address>"),
            (
                "bind [fe80::1c2d:3e4f]:4000 failed",
                "bind <address> failed",
            ),
            ("peer 2001:db8::5 went away", "peer <address> went away"),
            ("asked ::1: no answer", "asked <address> no answer"),
            (
                "candidate udp/192.168.1.20/50000 unusable",
                "candidate <address> unusable",
            ),
            (
                "no answer from 4f1e2d3c-aaaa.local:5353, giving up",
                "no answer from <address> giving up",
            ),
            ("Name.LOCAL not found", "<address> not found"),
            // What is no address stays: times, counts, versions of two parts.
            (
                "took 12:30:05, 3.5 s over, v1.2",
                "took 12:30:05, 3.5 s over, v1.2",
            ),
            ("os error 49: cannot assign", "os error 49: cannot assign"),
        ] {
            assert_eq!(discreet(said), logged, "{said}");
        }
    }

    #[test]
    fn a_log_written_where_the_user_said_is_whole() {
        struct Shared(std::sync::Arc<Mutex<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let written = std::sync::Arc::new(Mutex::new(Vec::new()));
        let mut out = Out::Given(Box::new(Shared(written.clone())));
        // Far more than a kept log holds.
        let total = MAX_LINES * 3;
        for i in 0..total {
            out.push(format!("line {i}"));
        }
        let text = String::from_utf8(written.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), total);
        assert!(!text.contains("left out"));
        assert_eq!(
            text.lines().nth(MAX_LINES).unwrap(),
            format!("line {MAX_LINES}")
        );
    }

    #[test]
    fn a_long_log_keeps_its_beginning_and_its_end() {
        let dir = std::env::temp_dir().join(format!("qrsend-log-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long.log");
        let header = "header\n\n".to_string();
        let mut kept = Kept {
            file: Some(File::create(&path).unwrap()),
            path: path.clone(),
            header: header.clone(),
            first: Vec::new(),
            rest: VecDeque::new(),
            dropped: 0,
        };
        fs::write(&path, &header).unwrap();
        let total = MAX_LINES * 3;
        for i in 0..total {
            kept.push(format!("line {i}"));
        }
        drop(kept);
        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // The header and its blank line, the lines, and the note of the gap.
        assert!(lines.len() <= MAX_LINES + 3, "{} lines", lines.len());
        assert_eq!(lines[0], "header");
        assert_eq!(lines[2], "line 0");
        assert_eq!(
            lines[2 + KEEP_FIRST - 1],
            format!("line {}", KEEP_FIRST - 1)
        );
        assert!(lines[2 + KEEP_FIRST].contains("lines left out"));
        assert_eq!(*lines.last().unwrap(), format!("line {}", total - 1));
        // Nothing is there twice, and what is left out is counted.
        let left_out: usize = lines[2 + KEEP_FIRST]
            .split_whitespace()
            .find_map(|w| w.parse().ok())
            .unwrap();
        assert_eq!(lines.len() - 3 + left_out, total);
        let _ = fs::remove_dir_all(dir);
    }
}
