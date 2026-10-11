//! `qrsend storage`: what QRSend keeps on this machine, how much room each
//! thing takes, and removing it — all of it, by kind, by age, or picked from
//! the list.
//!
//! Three kinds of things are kept: transfers received (the inbox: unfinished
//! ones, and finished ones kept with `--keep` or whose saving failed),
//! transfers sent (packed once, kept two weeks to be sent again) and the logs
//! of the last runs. The device ID and the trusted devices are settings, not
//! storage: they are shown, and never removed here.

use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;

use crate::spool::{MAX_AGE_SECS, NOT_KEPT, SpoolInfo};
use crate::store::State;
use crate::{identity, log, paths, util};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inbox,
    Sent,
    Logs,
}

const KINDS: [Kind; 3] = [Kind::Inbox, Kind::Sent, Kind::Logs];

impl Kind {
    fn title(self) -> &'static str {
        match self {
            Kind::Inbox => "Received (the inbox)",
            Kind::Sent => "Sent (kept two weeks, to be sent again)",
            Kind::Logs => "Logs of the last runs",
        }
    }

    fn dir(self) -> PathBuf {
        match self {
            Kind::Inbox => paths::sessions_dir(),
            Kind::Sent => paths::spools_dir(),
            Kind::Logs => paths::logs_dir(),
        }
    }
}

/// One thing that can be removed.
struct Item {
    kind: Kind,
    /// What it is called in the list and on the command line.
    id: String,
    path: PathBuf,
    /// Room taken on disk.
    size: u64,
    /// When it was last written (seconds since 1970).
    modified: i64,
    what: String,
}

/// Everything kept, in the order it is listed.
fn items() -> Vec<Item> {
    let mut out = Vec::new();
    for kind in [Kind::Inbox, Kind::Sent] {
        let mut found: Vec<Item> = fs::read_dir(kind.dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| {
                let path = e.path();
                let (size, modified) = util::disk_use(&path);
                let what = match kind {
                    Kind::Inbox => received(&path),
                    _ => sent(&path),
                };
                Item {
                    kind,
                    id: util::printable(&e.file_name().to_string_lossy()),
                    what: what.unwrap_or_else(|| "left over, of no use".into()),
                    path,
                    size,
                    modified,
                }
            })
            .collect();
        found.sort_by_key(|i| i.modified);
        out.extend(found);
    }
    let logs = log::kept().len();
    if logs > 0 {
        let path = paths::logs_dir();
        let (size, modified) = util::disk_use(&path);
        out.push(Item {
            kind: Kind::Logs,
            id: "logs".into(),
            path,
            size,
            modified,
            what: format!("{logs} log{}", if logs == 1 { "" } else { "s" }),
        });
    }
    out
}

/// What a session in the inbox is: how far it got, and of what.
fn received(dir: &std::path::Path) -> Option<String> {
    let state: State = serde_json::from_slice(&fs::read(dir.join("state.json")).ok()?).ok()?;
    let total = state.seg_count as usize + 1;
    let done = util::parse_ranges(&state.done).ok()?.len();
    let status = if done == total && state.unverified.is_empty() {
        "complete".to_string()
    } else {
        format!("{done}/{total} parts")
    };
    // (The summary is from the sender: nothing in it may drive the terminal.)
    Some(format!(
        "{status} · {}",
        state
            .summary
            .as_deref()
            .map_or("(file list not received yet)".into(), util::printable)
    ))
}

/// What a spool is, and how long it stays.
fn sent(dir: &std::path::Path) -> Option<String> {
    let info: SpoolInfo = serde_json::from_slice(&fs::read(dir.join("spool.json")).ok()?).ok()?;
    let left = MAX_AGE_SECS - (util::now() - info.created);
    Some(format!(
        "{} · {}",
        util::printable(&info.summary),
        match left {
            // (`send --no-keep`: still sending, or killed before it removed it.)
            _ if dir.join(NOT_KEPT).exists() => {
                "not kept: removed when its send ends, at the latest at the next send".to_string()
            }
            i64::MIN..=0 => "past its two weeks: removed at the next send".to_string(),
            1..=86_400 => "removed within a day".to_string(),
            _ => format!("removed in {} days", (left + 86_399) / 86_400),
        }
    ))
}

fn total(items: &[&Item]) -> u64 {
    items.iter().map(|i| i.size).sum()
}

/// Prints the list, numbered as [`pick`] understands it.
fn print(items: &[Item]) {
    let all: Vec<&Item> = items.iter().collect();
    if items.is_empty() {
        println!("QRSend keeps nothing on this machine.");
    } else {
        println!(
            "QRSend keeps {} on this machine.",
            util::human_bytes(total(&all))
        );
    }
    let now = util::now();
    let mut n = 0;
    for kind in KINDS {
        let of_kind: Vec<&Item> = items.iter().filter(|i| i.kind == kind).collect();
        println!(
            "\n{} — {}\n  in {}",
            kind.title(),
            if of_kind.is_empty() {
                "nothing".to_string()
            } else {
                util::human_bytes(total(&of_kind))
            },
            kind.dir().display()
        );
        for item in of_kind {
            n += 1;
            println!(
                "{n:>4}  {:<8}  {:>10}  {:<11}  {}",
                item.id,
                util::human_bytes(item.size),
                util::human_age(now - item.modified),
                item.what
            );
        }
    }
    // (Only these two files: on some systems the inbox is in the same directory.)
    let settings = [identity::identity_path(), identity::devices_path()];
    if settings.iter().any(|p| p.exists()) {
        println!(
            "\nNot counted, and never removed here: this device's ID and the trusted devices ({}, in {}).",
            util::human_bytes(settings.iter().map(|p| util::disk_use(p).0).sum()),
            paths::config_dir().display()
        );
    }
}

#[derive(clap::Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct StorageArgs {
    #[command(subcommand)]
    cmd: Option<StorageCmd>,
}

#[derive(Subcommand)]
enum StorageCmd {
    /// List what is kept, with the room each thing takes (what `qrsend
    /// storage` alone does)
    List,
    /// Remove what is kept. Without options: shows the list and asks which
    Clean(CleanArgs),
}

#[derive(clap::Args)]
struct CleanArgs {
    /// Remove only these (IDs as `qrsend storage` lists them)
    #[arg(value_name = "ID")]
    ids: Vec<String>,
    /// Remove everything listed (never the device ID or the trusted devices)
    #[arg(long, conflicts_with_all = ["ids", "inbox", "sent", "logs"])]
    all: bool,
    /// Received transfers (the inbox)
    #[arg(long)]
    inbox: bool,
    /// Transfers kept to be sent again
    #[arg(long)]
    sent: bool,
    /// Logs of past runs
    #[arg(long)]
    logs: bool,
    /// Only what was last written this long ago or longer: 7d, 12h, 30m (a
    /// plain number is days). Alone, or to narrow the other choices
    #[arg(long, value_name = "AGE", value_parser = parse_age)]
    older_than: Option<i64>,
    /// Remove without asking
    #[arg(short, long)]
    yes: bool,
    /// Say what would be removed, and remove nothing
    #[arg(long)]
    dry_run: bool,
}

/// An age in seconds from "7d", "12h", "30m" or a number of days.
fn parse_age(s: &str) -> Result<i64, String> {
    let bad = || format!("invalid age {s:?} (use for example 7d, 12h or 30m)");
    let (number, unit) = match s.char_indices().last() {
        Some((i, 'd')) => (&s[..i], 86_400),
        Some((i, 'h')) => (&s[..i], 3_600),
        Some((i, 'm')) => (&s[..i], 60),
        _ => (s, 86_400),
    };
    let number: u32 = number.parse().map_err(|_| bad())?;
    Ok(i64::from(number) * unit)
}

/// The numbers of a line like "1 3-5, 7" or "all", each from 1 to `count`.
fn pick(line: &str, count: usize) -> Result<Vec<usize>, String> {
    let line = line.trim().to_ascii_lowercase();
    if line == "all" || line == "a" {
        return Ok((1..=count).collect());
    }
    let mut out = Vec::new();
    for word in line.split([' ', ',']).filter(|w| !w.is_empty()) {
        let number = |w: &str| {
            w.parse::<usize>()
                .ok()
                .filter(|n| (1..=count).contains(n))
                .ok_or_else(|| format!("{word:?} is not a number from 1 to {count}"))
        };
        let (from, to) = match word.split_once('-') {
            Some((a, b)) => (number(a)?, number(b)?),
            None => (number(word)?, number(word)?),
        };
        if from > to {
            return Err(format!("{word:?} runs backward"));
        }
        out.extend(from..=to);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// One line typed at the terminal, or nothing at the end of input.
fn ask(prompt: &str) -> Result<Option<String>> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    Ok((std::io::stdin().lock().read_line(&mut answer)? > 0).then_some(answer))
}

fn clean(args: CleanArgs) -> Result<()> {
    let items = items();
    if items.is_empty() {
        println!("QRSend keeps nothing on this machine: there is nothing to remove.");
        return Ok(());
    }
    let interactive = std::io::stdin().is_terminal();
    let kinds: Vec<Kind> = [
        (args.inbox, Kind::Inbox),
        (args.sent, Kind::Sent),
        (args.logs, Kind::Logs),
    ]
    .into_iter()
    .filter_map(|(on, kind)| on.then_some(kind))
    .collect();
    let said = args.all || !kinds.is_empty() || !args.ids.is_empty() || args.older_than.is_some();
    let chosen: Vec<&Item> = if said {
        if let Some(unknown) = args
            .ids
            .iter()
            .find(|id| !items.iter().any(|i| i.id == **id))
        {
            bail!("nothing is kept under {unknown:?} (see `qrsend storage`)");
        }
        let now = util::now();
        items
            .iter()
            .filter(|i| kinds.is_empty() || kinds.contains(&i.kind))
            .filter(|i| args.ids.is_empty() || args.ids.contains(&i.id))
            .filter(|i| args.older_than.is_none_or(|age| now - i.modified >= age))
            .collect()
    } else {
        if !interactive {
            bail!(
                "say what to remove: --all, --inbox, --sent, --logs, --older-than AGE, or IDs (see `qrsend storage`)"
            );
        }
        print(&items);
        loop {
            let Some(line) = ask(
                "\nRemove which? Numbers (like 1 3-5), \"all\", or just Enter to remove nothing: ",
            )?
            else {
                return Ok(());
            };
            match pick(&line, items.len()) {
                Ok(numbers) => break numbers.into_iter().map(|n| &items[n - 1]).collect(),
                Err(e) => eprintln!("{e}."),
            }
        }
    };
    if chosen.is_empty() {
        println!("Nothing was removed.");
        return Ok(());
    }
    let now = util::now();
    for item in &chosen {
        println!(
            "  {:<8}  {:>10}  {:<11}  {}",
            item.id,
            util::human_bytes(item.size),
            util::human_age(now - item.modified),
            item.what
        );
    }
    let sum = format!(
        "{} thing{} ({})",
        chosen.len(),
        if chosen.len() == 1 { "" } else { "s" },
        util::human_bytes(total(&chosen))
    );
    if args.dry_run {
        println!("Would remove {sum}. Nothing was removed.");
        return Ok(());
    }
    if !args.yes {
        if !interactive {
            bail!("add --yes to remove without being asked");
        }
        let answer = ask(&format!("Remove {sum}? This cannot be undone. [y/N] "))?;
        if !matches!(
            answer.as_deref().map(str::trim),
            Some("y" | "Y" | "yes" | "Yes")
        ) {
            println!("Nothing was removed.");
            return Ok(());
        }
    }
    let (mut freed, mut failed) = (0, 0);
    for item in &chosen {
        let removed = if item.path.is_dir() {
            fs::remove_dir_all(&item.path)
        } else {
            fs::remove_file(&item.path)
        };
        match removed {
            Ok(()) => freed += item.size,
            Err(e) => {
                failed += 1;
                eprintln!(
                    "Could not remove {}: {e}",
                    util::printable(&item.path.display().to_string())
                );
            }
        }
    }
    println!("Removed {}.", util::human_bytes(freed));
    if failed > 0 {
        bail!("{failed} of {} could not be removed", chosen.len());
    }
    Ok(())
}

pub fn run(args: StorageArgs) -> Result<()> {
    match args.cmd.unwrap_or(StorageCmd::List) {
        StorageCmd::List => {
            let items = items();
            print(&items);
            if !items.is_empty() {
                println!("\nTo make room: qrsend storage clean");
            }
            Ok(())
        }
        StorageCmd::Clean(args) => clean(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(parse_age("7d"), Ok(7 * 86_400));
        assert_eq!(parse_age("3"), Ok(3 * 86_400));
        assert_eq!(parse_age("12h"), Ok(12 * 3_600));
        assert_eq!(parse_age("30m"), Ok(1_800));
        assert_eq!(parse_age("0"), Ok(0));
        assert!(parse_age("").is_err());
        assert!(parse_age("d").is_err());
        assert!(parse_age("-1d").is_err());
        assert!(parse_age("soon").is_err());
    }

    #[test]
    fn picking_from_the_list() {
        assert_eq!(pick("1 3-5, 7", 8), Ok(vec![1, 3, 4, 5, 7]));
        assert_eq!(pick("2,2 1-2", 3), Ok(vec![1, 2]));
        assert_eq!(pick(" All \n", 3), Ok(vec![1, 2, 3]));
        assert_eq!(pick("\n", 3), Ok(vec![]));
        assert!(pick("0", 3).is_err());
        assert!(pick("4", 3).is_err());
        assert!(pick("3-1", 3).is_err());
        assert!(pick("two", 3).is_err());
    }
}
