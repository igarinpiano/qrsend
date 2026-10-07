//! `qrsend recv`: scan frames, rebuild segments, save and extract.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossbeam_channel::{bounded, unbounded};
use indicatif::{ProgressBar, ProgressStyle};
use qrsend_core::crypto::{self, DeviceIdentity, OpenMetaError};
use qrsend_core::frame::{FLAG_ENCRYPTED, Frame};
use qrsend_core::manifest::{Manifest, session_hex};
use qrsend_core::payload::segment_hash;
use qrsend_core::receiver::{Event, Receiver, SessionParams};
use qrsend_core::resume::ResumeCode;

use crate::extract::{self, Conflict, ExtractOptions, Outcome};
use crate::input::{Input, LumaFrame, image_paths};
use crate::store::Store;
use crate::{decode, identity, util};

#[derive(clap::Args)]
pub struct RecvArgs {
    /// Read frames from image files or directories of images (PNG/JPEG)
    #[arg(long, num_args = 1.., value_name = "PATH", conflicts_with = "video")]
    pub images: Vec<PathBuf>,
    /// Capture from a camera (through ffmpeg). Optional device: index on
    /// macOS (`ffmpeg -f avfoundation -list_devices true -i ""`), /dev/videoN on
    /// Linux, the device name on Windows
    #[arg(long, value_name = "DEVICE", num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["images", "video"])]
    pub camera: Option<String>,
    /// Read frames from a video (.y4m natively; other formats through ffmpeg)
    #[arg(long, value_name = "FILE")]
    pub video: Option<PathBuf>,
    /// Directory to save received files into
    #[arg(short, long, default_value = ".", value_name = "DIR")]
    pub out: PathBuf,
    /// Write a single received file (or text) to standard output
    #[arg(long, conflicts_with = "copy")]
    pub stdout: bool,
    /// Copy received text to the clipboard
    #[arg(long)]
    pub copy: bool,
    /// What to do when a received name already exists
    #[arg(long, value_enum, default_value_t = Conflict::Rename)]
    pub on_conflict: Conflict,
    /// Keep the session in the inbox after saving
    #[arg(long)]
    pub keep: bool,
    /// Only accept this session (8 hex digits)
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,
    /// Decoder threads (default: all cores)
    #[arg(long)]
    pub threads: Option<usize>,
}

/// Who sent a transfer, as far as signatures can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderInfo {
    Trusted(String),
    Unverified(String),
    Unsigned,
}

impl SenderInfo {
    pub fn describe(&self, claimed: Option<&str>) -> String {
        let claimed = claimed
            .map(|c| format!(" claims to be {c:?}"))
            .unwrap_or_default();
        match self {
            SenderInfo::Trusted(name) => format!("{name} ✓"),
            SenderInfo::Unverified(key) => format!("unverified sender (key {key}){claimed}"),
            SenderInfo::Unsigned => format!("unsigned{claimed}"),
        }
    }
}

/// Decrypts (if needed) and verifies a meta segment.
pub fn open_meta(
    bytes: &[u8],
    p: &SessionParams,
    me: Option<&DeviceIdentity>,
) -> Result<(Manifest, SenderInfo)> {
    let opened = match crypto::open_meta(
        bytes,
        p.session_id,
        p.flags & FLAG_ENCRYPTED != 0,
        me.map(|m| m.as_age()),
    ) {
        Err(OpenMetaError::NoIdentity) => {
            bail!(
                "this transfer is encrypted, but this device has no identity yet (run `qrsend id`)"
            )
        }
        Err(OpenMetaError::NotForUs) => bail!(
            "this transfer is encrypted for another device (remove it with `qrsend inbox rm {}`)",
            session_hex(p.session_id)
        ),
        r => r?,
    };
    let sender = match opened.signer {
        None => SenderInfo::Unsigned,
        Some(key) => match identity::trusted_name(&key) {
            Some(name) => SenderInfo::Trusted(name),
            None => SenderInfo::Unverified(crypto::key_id(&key)),
        },
    };
    Ok((opened.manifest, sender))
}

/// A receive session: the on-disk store plus the decoded manifest.
pub struct Session {
    pub store: Store,
    pub manifest: Option<Manifest>,
    pub sender: SenderInfo,
    pub me: Option<DeviceIdentity>,
}

impl Session {
    pub fn open(store: Store) -> Result<Session> {
        let me = identity::load()?;
        let mut s = Session {
            store,
            manifest: None,
            sender: SenderInfo::Unsigned,
            me,
        };
        if s.store.has_meta() {
            let (m, sender) = open_meta(&s.store.meta_bytes()?, &s.store.params(), s.me.as_ref())?;
            s.manifest = Some(m);
            s.sender = sender;
        }
        Ok(s)
    }

    fn seg_len(&self, index: u32) -> Option<usize> {
        let m = self.manifest.as_ref()?;
        let nominal = 1u64 << self.store.state.seg_shift;
        let start = (index as u64 - 1) * nominal;
        Some(m.body.length.saturating_sub(start).min(nominal) as usize)
    }

    fn check_manifest(&self, m: &Manifest) -> Result<()> {
        let p = self.store.params();
        let expected = m.body.length.div_ceil(1 << p.seg_shift);
        if expected != p.seg_count as u64 || m.body.segment_blake3.len() != p.seg_count as usize {
            bail!("manifest does not match the transfer layout");
        }
        Ok(())
    }

    /// Stores the meta segment; returns segments that failed late verification.
    fn on_meta(&mut self, data: &[u8]) -> Result<Vec<u32>> {
        self.store.write_meta(data)?;
        let (manifest, sender) = open_meta(data, &self.store.params(), self.me.as_ref())?;
        self.sender = sender;
        self.check_manifest(&manifest)?;
        self.store.state.summary = Some(summary(&manifest));
        self.manifest = Some(manifest);
        let mut bad = Vec::new();
        for index in self.store.take_unverified() {
            let len = self.seg_len(index).unwrap();
            let data = self.store.read_segment(index, len)?;
            if !self.verify(index, &data) {
                self.store.forget(index);
                bad.push(index);
            }
        }
        self.store.save()?;
        Ok(bad)
    }

    fn verify(&self, index: u32, data: &[u8]) -> bool {
        match &self.manifest {
            Some(m) => m
                .body
                .segment_blake3
                .get(index as usize - 1)
                .is_some_and(|h| *h == segment_hash(data)),
            None => true,
        }
    }

    /// Stores a body segment; returns false if it failed verification.
    fn on_segment(&mut self, index: u32, data: &[u8]) -> Result<bool> {
        if !self.verify(index, data) {
            return Ok(false);
        }
        self.store
            .write_segment(index, data, self.manifest.is_some())?;
        Ok(true)
    }
}

pub fn summary(m: &Manifest) -> String {
    let top: Vec<&str> = m
        .entries
        .iter()
        .filter(|e| !e.path.contains('/'))
        .map(|e| e.path.as_str())
        .collect();
    match m.kind {
        qrsend_core::manifest::Kind::Text => {
            format!("text, {}", util::human_bytes(m.body.plain_length))
        }
        qrsend_core::manifest::Kind::Files => {
            let names = match top.len() {
                0 => String::from("(empty)"),
                1 => top[0].to_string(),
                n => format!("{} and {} more", top[0], n - 1),
            };
            let files = m.file_count();
            format!(
                "{names} — {files} file{}, {}",
                if files == 1 { "" } else { "s" },
                util::human_bytes(m.body.plain_length)
            )
        }
    }
}

pub fn print_manifest_header(
    pb: &ProgressBar,
    m: &Manifest,
    p: &SessionParams,
    sender: &SenderInfo,
) {
    say(
        pb,
        format!("Session {} · {}", session_hex(p.session_id), summary(m)),
    );
    say(
        pb,
        format!("From:   {}", sender.describe(m.sender_name.as_deref())),
    );
    if p.flags & FLAG_ENCRYPTED != 0 {
        say(pb, "Encrypted for this device.");
    }
    say(
        pb,
        format!(
            "On the wire: {} in {} segment{}",
            util::human_bytes(m.body.length),
            p.seg_count,
            if p.seg_count == 1 { "" } else { "s" }
        ),
    );
}

pub fn resume_hint(session_id: u32, missing: &[u32]) -> String {
    let code = ResumeCode::new(session_id, missing.to_vec()).encode();
    format!(
        "{} segment{} still missing. On the sender run:\n  qrsend send --resume {code}\nthen run `qrsend recv` again (progress is saved).",
        missing.len(),
        if missing.len() == 1 { "" } else { "s" }
    )
}

pub fn report(outcome: Outcome, stdout_text: bool) {
    match outcome {
        Outcome::Text(text) => {
            if stdout_text {
                print!("{text}");
            } else {
                println!("{text}");
            }
        }
        Outcome::Stdout => {}
        Outcome::Files { saved, skipped } => {
            for p in &saved {
                eprintln!("Saved {}", p.display());
            }
            for p in &skipped {
                eprintln!("Skipped {} (already exists)", p.display());
            }
        }
    }
}

pub fn run(args: RecvArgs) -> Result<()> {
    let input = if let Some(dev) = &args.camera {
        let dev = if dev.is_empty() {
            crate::input::default_camera().to_string()
        } else {
            dev.clone()
        };
        eprintln!(
            "Scanning camera {dev:?} — point it at the sender's screen (Ctrl-C to stop; progress is saved)."
        );
        Input::Camera(dev)
    } else if let Some(v) = &args.video {
        Input::Video(v.clone())
    } else if !args.images.is_empty() {
        Input::Images(image_paths(&args.images)?)
    } else {
        bail!("choose an input: --camera, --video FILE or --images PATH");
    };
    let threads = args
        .threads
        .unwrap_or_else(|| {
            thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        })
        .max(1);

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || stop.store(true, Ordering::SeqCst));
    }

    let (ftx, frx) = bounded::<LumaFrame>(threads * 2);
    let (ttx, trx) = unbounded::<Vec<String>>();
    let producer = thread::spawn(move || input.produce(ftx));
    let lattice = Arc::new(std::sync::Mutex::new(decode::Lattice::default()));
    for _ in 0..threads {
        let (frx, ttx, stop, lattice) = (frx.clone(), ttx.clone(), stop.clone(), lattice.clone());
        thread::spawn(move || {
            while let Ok(f) = frx.recv() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let texts = decode::detect_cached(&f, &lattice);
                if ttx.send(texts).is_err() {
                    break;
                }
            }
        });
    }
    drop((frx, ttx));

    let mut rx = Receiver::new();
    if let Some(s) = &args.session {
        rx = rx.expect_session(util::parse_session(s)?);
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::with_template("{spinner} {msg}").unwrap());
    pb.enable_steady_tick(Duration::from_millis(120));
    pb.set_message("Waiting for QRSend frames…");

    let mut session: Option<Session> = None;
    let mut scanned = 0u64;
    let mut last_save = Instant::now();
    let mut meter = util::RateMeter::new(Duration::from_secs(5));
    let mut last_log = Instant::now();

    'outer: for texts in trx.iter() {
        scanned += 1;
        for text in texts {
            let Ok(frame) = Frame::from_qr_text(&text) else {
                continue;
            };
            for event in rx.push(frame) {
                match event {
                    Event::Locked(p) => {
                        let store = Store::open_or_create(p)?;
                        for i in store.done_indices() {
                            rx.mark_done(i);
                        }
                        let resumed = !store.done_indices().is_empty();
                        let s = Session::open(store)?;
                        if let Some(m) = &s.manifest {
                            print_manifest_header(&pb, m, &p, &s.sender);
                        } else {
                            say(
                                &pb,
                                format!(
                                    "Session {} · {} segments",
                                    session_hex(p.session_id),
                                    p.seg_count + 1
                                ),
                            );
                        }
                        if resumed {
                            say(
                                &pb,
                                format!(
                                    "Resuming: {}/{} segments already received",
                                    rx.completed_count(),
                                    rx.segment_total()
                                ),
                            );
                        }
                        pb.set_style(
                            ProgressStyle::with_template("{bar:16.cyan/blue} {wide_msg}").unwrap(),
                        );
                        pb.set_length(1000);
                        session = Some(s);
                    }
                    Event::ForeignSession(id) => {
                        say(
                            &pb,
                            format!("Ignoring frames of another session ({})", session_hex(id)),
                        );
                    }
                    Event::Inconsistent => {}
                    Event::Completed { index, data } => {
                        let s = session.as_mut().expect("locked before completion");
                        if index == 0 {
                            for bad in s.on_meta(&data)? {
                                rx.reset(bad);
                            }
                            let m = s.manifest.as_ref().unwrap();
                            print_manifest_header(&pb, m, &s.store.params(), &s.sender);
                        } else if !s.on_segment(index, &data)? {
                            rx.reset(index);
                            say(
                                &pb,
                                format!(
                                    "Segment {index} failed verification; waiting for it again"
                                ),
                            );
                        }
                        if last_save.elapsed() > Duration::from_secs(1) {
                            s.store.save()?;
                            last_save = Instant::now();
                        }
                    }
                }
            }
        }
        if session.is_some()
            && let Some(p) = rx.progress()
        {
            // What matters while waiting: how much is left and how fast it goes.
            let rate = meter.update(Instant::now(), p.remaining_bytes);
            let done = p.total_bytes - p.remaining_bytes;
            pb.set_position(done * 1000 / p.total_bytes.max(1));
            let eta = if rate > 0.0 {
                format!(
                    " · {} left",
                    util::human_duration(p.remaining_bytes as f64 / rate)
                )
            } else {
                String::new()
            };
            let line = format!(
                "{}% · {} of {} · {} of {} codes, {} to go · {}/s{eta}",
                done * 100 / p.total_bytes.max(1),
                util::human_bytes(done),
                util::human_bytes(p.total_bytes),
                p.total_symbols - p.remaining_symbols,
                p.total_symbols,
                p.remaining_symbols,
                util::human_bytes(rate as u64),
            );
            // Without a terminal there is no bar; log a line now and then.
            if pb.is_hidden() && last_log.elapsed() >= Duration::from_secs(2) {
                eprintln!("{line}");
                last_log = Instant::now();
            }
            pb.set_message(line);
        }
        if rx.is_complete() || stop.load(Ordering::Relaxed) {
            break 'outer;
        }
    }
    stop.store(true, Ordering::SeqCst);
    pb.finish_and_clear();

    let Some(mut s) = session else {
        // Surface input errors (unreadable file, missing ffmpeg) first.
        if producer.is_finished() {
            producer.join().expect("input thread panicked")?;
        }
        bail!("no QRSend frames found in {scanned} image(s)");
    };
    s.store.save()?;
    let id = s.store.state.session_id;
    if !s.store.is_complete() {
        eprintln!(
            "Received {}/{} segments.",
            rx.completed_count(),
            rx.segment_total()
        );
        eprintln!("{}", resume_hint(id, &s.store.missing()));
        std::process::exit(2);
    }
    let manifest = s.manifest.clone().expect("complete session has a manifest");
    let opts = ExtractOptions {
        out: args.out.clone(),
        conflict: args.on_conflict,
        stdout: args.stdout,
        copy: args.copy,
    };
    match extract::finalize(&s.store, &manifest, s.me.as_ref(), &opts) {
        Ok(outcome) => {
            let text = match &outcome {
                Outcome::Text(t) => Some(t.clone()),
                _ => None,
            };
            report(outcome, args.stdout);
            if args.copy {
                eprintln!("Copied to the clipboard.");
            } else if let Some(text) = text.filter(|_| !args.stdout) {
                offer_text_actions(&text, &args.out, id)?;
            }
            if args.keep {
                eprintln!("Kept in the inbox as {}.", session_hex(id));
            } else {
                s.store.remove()?;
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("Received everything, but saving failed. The data is kept in the inbox:");
            eprintln!("  qrsend inbox export {} -o <dir>", session_hex(id));
            Err(e)
        }
    }
}

/// After a text arrives in an interactive terminal, lets the user copy it or
/// save it without having asked for that up front.
fn offer_text_actions(text: &str, out: &std::path::Path, session_id: u32) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        return Ok(());
    }
    loop {
        eprint!("[c] copy to clipboard  [s] save as file  [Enter] done: ");
        std::io::stderr().flush()?;
        let mut answer = String::new();
        if std::io::stdin().lock().read_line(&mut answer)? == 0 {
            return Ok(());
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "c" => match extract::copy_to_clipboard(text) {
                Ok(()) => eprintln!("Copied to the clipboard."),
                Err(e) => eprintln!("Could not copy: {e}"),
            },
            "s" => {
                std::fs::create_dir_all(out)?;
                let path =
                    extract::free_path(out, &format!("qrsend-{}.txt", session_hex(session_id)));
                std::fs::write(&path, text)?;
                eprintln!("Saved {}", path.display());
            }
            "" => return Ok(()),
            _ => {}
        }
    }
}

/// Prints above the progress bar, or plainly when the bar is hidden (not a TTY).
pub fn say(pb: &ProgressBar, msg: impl AsRef<str>) {
    if pb.is_hidden() {
        eprintln!("{}", msg.as_ref());
    } else {
        pb.println(msg);
    }
}
