//! `qrsend recv`: scan frames, rebuild segments, save and extract.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use crossbeam_channel::{bounded, never, select, tick, unbounded};
use indicatif::{ProgressBar, ProgressStyle};
use qrsend_core::crypto::{self, DeviceIdentity, OpenMetaError};
use qrsend_core::direct::{self, Record};
use qrsend_core::frame::{FLAG_ENCRYPTED, Frame};
use qrsend_core::link::{self, TcpOffer};
use qrsend_core::manifest::{Manifest, session_hex};
use qrsend_core::payload::segment_hash;
use qrsend_core::receiver::{Event, Receiver, SessionParams};
use qrsend_core::resume::ResumeCode;

#[cfg(feature = "window")]
use crate::display;
use crate::extract::{self, Conflict, ExtractOptions, Outcome};
use crate::input::{Input, Picture, image_paths};
#[cfg(feature = "webrtc")]
use crate::rtc;
use crate::send::parse_size;
use crate::store::Store;
use crate::{decode, identity, log, net, util};

#[derive(clap::Args)]
pub struct RecvArgs {
    /// The strictest way to receive: only transfers encrypted for this
    /// device and signed by a trusted one are taken (anything else is
    /// refused, and what was stored of it removed); one way only (no network
    /// connection is made, nothing is told back to the sender); and nothing
    /// is left besides what is saved (no log, nothing kept in the inbox,
    /// not even of a transfer that stopped halfway, which then starts over;
    /// nothing put on the clipboard). Options that go against any of
    /// this are errors. Setting QRSEND_SECURE=1 in the environment makes
    /// this the rule
    #[arg(long, env = "QRSEND_SECURE", value_parser = clap::builder::FalseyValueParser::new())]
    pub secure: bool,
    /// Read frames from image files or directories of images (PNG/JPEG)
    #[arg(long, num_args = 1.., value_name = "PATH", conflicts_with = "video")]
    pub images: Vec<PathBuf>,
    /// Capture from a camera (through ffmpeg). Optional device: index on
    /// macOS (`ffmpeg -f avfoundation -list_devices true -i ""`), /dev/videoN on
    /// Linux, the device name on Windows
    #[arg(long, value_name = "DEVICE", num_args = 0..=1, default_missing_value = "", conflicts_with_all = ["images", "video"])]
    pub camera: Option<String>,
    /// Picture size to ask the camera for, e.g. 1280x720. Without it the
    /// camera's default is used, which is often small (640x480 on Linux):
    /// dense codes need more
    #[arg(long, value_name = "WxH", requires = "camera", value_parser = parse_size)]
    pub camera_size: Option<(usize, usize)>,
    /// Pictures per second to ask the camera for
    #[arg(long, value_name = "N", requires = "camera")]
    pub camera_fps: Option<f64>,
    /// Read frames from a video (.y4m natively; other formats through ffmpeg)
    #[arg(long, value_name = "FILE")]
    pub video: Option<PathBuf>,
    /// Read frames as text, one per line ("-" = standard input), as written
    /// by `qrsend send --export-text` — from a serial line, a TCP connection,
    /// ssh, a file…
    #[arg(long, value_name = "FILE", conflicts_with_all = ["images", "video", "camera"])]
    pub text: Option<PathBuf>,
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
    /// Do not connect to a sender that offers a network connection
    /// (`qrsend send --lan`); read the codes only
    #[arg(long)]
    pub no_lan: bool,
    /// Two-way transfer over whatever carries bytes: write feedback codes
    /// (what is still missing, and at the end that everything arrived) as
    /// text, one per line, to FILE ("-": standard output), for a sender
    /// started with `qrsend send --feedback-from`
    #[arg(long, value_name = "FILE", conflicts_with = "stdout")]
    pub feedback_to: Option<PathBuf>,
    /// Forward secrecy: before reading, make a one-time key and show it for
    /// the sender (as text, and as codes for its camera). The sender
    /// encrypts to it as well (`qrsend send --one-time-key`, or the web
    /// app's Send page), and this device destroys it once the transfer has
    /// arrived: after that a recording of the codes cannot be read even
    /// with this device's keys. A key nobody used is destroyed after a day
    #[arg(long)]
    pub one_time_key: bool,
    /// Two-way transfer with a sender that has a camera (the web app with
    /// "Two-way transfer" on, or `qrsend send --two-way`): show feedback
    /// codes in a window for that camera to read. The sender then sends
    /// only what is still missing and stops when everything has arrived
    #[arg(long)]
    pub two_way: bool,
    /// Decoder threads (default: all cores)
    #[arg(long)]
    pub threads: Option<usize>,
    /// Write the diagnostic log to FILE ("-": standard error) instead of
    /// keeping it for `qrsend log`. It holds no file names, contents, device
    /// IDs or network addresses
    #[arg(long, value_name = "FILE")]
    pub log: Option<PathBuf>,
}

/// Who sent a transfer, as far as signatures can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SenderInfo {
    /// A trusted device; `pq`: known with its post-quantum keys, so that
    /// the signature holds against a quantum computer as well.
    Trusted {
        name: String,
        pq: bool,
    },
    Unverified(String),
    Unsigned,
}

impl SenderInfo {
    pub fn describe(&self, claimed: Option<&str>) -> String {
        let claimed = claimed
            .map(|c| format!(" claims to be {c:?}"))
            .unwrap_or_default();
        match self {
            SenderInfo::Trusted { name, pq } => format!(
                "{} ✓ ({})",
                util::printable(name),
                match pq {
                    true => "post-quantum signature",
                    false => "older signature only",
                }
            ),
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
            "this transfer is encrypted for another device, or for this one's older ID while \
             --secure takes only what post-quantum keys protect (remove it with `qrsend inbox rm {}`)",
            session_hex(p.session_id)
        ),
        r => r?,
    };
    let sender = match opened.signer {
        None => SenderInfo::Unsigned,
        Some(key) => match identity::trusted_device(&key) {
            // A device known with its post-quantum keys signs with both
            // schemes, always: with the older one alone, this is not it
            // (PROTOCOL §9.1).
            Some(device) => match opened.check(&device) {
                crypto::Signed::Downgraded => return Err(OpenMetaError::Downgraded.into()),
                _ => SenderInfo::Trusted {
                    name: device.name.clone(),
                    pq: device.pq.is_some(),
                },
            },
            None => SenderInfo::Unverified(crypto::key_id(&key)),
        },
    };
    Ok((opened.manifest, sender))
}

/// Why `--secure` does not take a transfer, if it does not: it takes
/// what is encrypted for this device and signed by a device it trusts.
/// `sender`: who signed, once the file list is open.
fn refused_as_insecure(
    p: &SessionParams,
    sender: Option<&SenderInfo>,
    protection: Option<crypto::Protection>,
) -> Option<&'static str> {
    if p.flags & FLAG_ENCRYPTED == 0 {
        return Some("it is not encrypted");
    }
    let by_sender = match sender {
        Some(SenderInfo::Unsigned) => Some("it is not signed"),
        Some(SenderInfo::Unverified(_)) => Some("its sender is not a trusted device"),
        Some(SenderInfo::Trusted { pq: false, .. }) => Some(
            "its sender is trusted with keys from before post-quantum ones only (have it show \
             its ID again, and add that)",
        ),
        Some(SenderInfo::Trusted { pq: true, .. }) | None => None,
    };
    if by_sender.is_some() {
        return by_sender;
    }
    // (Known with the file list, as the sender is.)
    if sender.is_some() && protection != Some(crypto::Protection::ForwardSecret) {
        return Some(
            "it was not encrypted to the one-time key this device showed (no forward secrecy)",
        );
    }
    None
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
        Session::open_as(store, false)
    }

    /// `strict`: this device opens only what was encrypted with both the
    /// older and the post-quantum scheme (`--secure`).
    pub fn open_as(store: Store, strict: bool) -> Result<Session> {
        let mut me = identity::load()?;
        if let (true, Some(me)) = (strict, &mut me) {
            me.require_pq();
        }
        // The one-time keys shown and not yet done with: what was encrypted
        // to one of them opens with it.
        if let Some(me) = &mut me {
            for (_, secret) in crate::once::pending() {
                me.hold_once(secret);
            }
        }
        let mut s = Session {
            store,
            manifest: None,
            sender: SenderInfo::Unsigned,
            me,
        };
        if s.store.has_meta() {
            match s.read_meta(&s.store.meta_bytes()?)? {
                Ok((m, sender)) => {
                    s.manifest = Some(m);
                    s.sender = sender;
                }
                // (Stored by an earlier version without being checked first.)
                Err(_) => s.store.forget(0),
            }
        }
        Ok(s)
    }

    /// Opens and checks a file list. The outer error is final (this device
    /// cannot open the transfer); the inner one says that the list is
    /// damaged or was tampered with, so collecting it again may bring a good
    /// one.
    fn read_meta(&self, data: &[u8]) -> Result<Result<(Manifest, SenderInfo)>> {
        match open_meta(data, &self.store.params(), self.me.as_ref()) {
            Ok((m, sender)) => Ok(self.check_manifest(&m).map(|()| (m, sender))),
            Err(e) if e.downcast_ref::<OpenMetaError>().is_some() => Ok(Err(e)),
            Err(e) => Err(e),
        }
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

    /// Stores the meta segment once it has been checked; returns segments
    /// that failed late verification, or (inner error) why the meta segment
    /// is of no use and has to be received again.
    fn on_meta(&mut self, data: &[u8]) -> Result<Result<Vec<u32>>> {
        let (manifest, sender) = match self.read_meta(data)? {
            Ok(opened) => opened,
            Err(e) => return Ok(Err(e)),
        };
        self.store.write_meta(data)?;
        self.sender = sender;
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
        Ok(Ok(bad))
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

/// A line about what a transfer holds (names as a terminal can show them).
pub fn summary(m: &Manifest) -> String {
    let top: Vec<String> = m
        .entries
        .iter()
        .filter(|e| !e.path.contains('/'))
        .map(|e| util::printable(&e.path))
        .collect();
    match m.kind {
        qrsend_core::manifest::Kind::Text => {
            format!("text, {}", util::human_bytes(m.body.plain_length))
        }
        qrsend_core::manifest::Kind::Files => {
            let names = match top.len() {
                0 => String::from("(empty)"),
                1 => top[0].clone(),
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

/// What protects the transfer a store holds, if it is encrypted and its
/// file list is here.
pub fn protection(store: &Store) -> Option<crypto::Protection> {
    if store.params().flags & FLAG_ENCRYPTED == 0 || !store.has_meta() {
        return None;
    }
    crypto::protection(&store.meta_bytes().ok()?).ok()
}

pub fn print_manifest_header(
    pb: &ProgressBar,
    m: &Manifest,
    p: &SessionParams,
    sender: &SenderInfo,
    protection: Option<crypto::Protection>,
) {
    say(
        pb,
        format!("Session {} · {}", session_hex(p.session_id), summary(m)),
    );
    say(
        pb,
        format!("From:   {}", sender.describe(m.sender_name.as_deref())),
    );
    // Which keys are at work is said every time: the older ones alone are a
    // state to get out of, and nobody gets out of what they do not see.
    if let SenderInfo::Trusted { name, pq: false } = sender {
        say(
            pb,
            format!(
                "WARNING: {} is trusted here with its older keys only, so its signature is checked \
                 the older way only (no protection against a quantum computer). Have it show its \
                 ID again and add that: qrsend devices add <ID>",
                util::printable(name)
            ),
        );
    }
    if p.flags & FLAG_ENCRYPTED != 0 {
        say(
            pb,
            match protection {
                Some(crypto::Protection::ForwardSecret) => {
                    "Encrypted for this device, with forward secrecy and post-quantum protection \
                     (one-time key, X25519 + ML-KEM-768)."
                }
                Some(crypto::Protection::PostQuantum) => {
                    "Encrypted for this device, with post-quantum protection (X25519 + ML-KEM-768). \
                     No forward secrecy: the sender was given no one-time key (recv --one-time-key)."
                }
                _ => {
                    "Encrypted for this device.\nWARNING: encrypted the older way only (X25519, no \
                     protection against a quantum computer): the sender knows this device, or \
                     another receiver of this transfer, by an ID from before post-quantum keys. \
                     Show the sender this device's ID again: qrsend id"
                }
            },
        );
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
        // Asked for as it is (--stdout), or going elsewhere than a terminal:
        // the text itself. On a terminal: what it says, not what it would
        // make the terminal do.
        Outcome::Text(text) if stdout_text => print!("{text}"),
        Outcome::Text(text) if !std::io::IsTerminal::is_terminal(&std::io::stdout()) => {
            println!("{text}")
        }
        Outcome::Text(text) => println!("{}", util::printable_text(&text)),
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

/// Reads frames as lines of text and passes them on in batches.
fn read_text(path: &std::path::Path, tx: crossbeam_channel::Sender<Vec<String>>) -> Result<()> {
    use std::io::BufRead;
    let reader: Box<dyn BufRead> = if path.as_os_str() == "-" {
        Box::new(std::io::stdin().lock())
    } else {
        Box::new(std::io::BufReader::new(std::fs::File::open(path).map_err(
            |e| anyhow::anyhow!("cannot open {}: {e}", path.display()),
        )?))
    };
    let mut batch = Vec::new();
    // Passed on by the batch when lines come fast, and one by one when they
    // come slowly (a serial line): a sender that waits for this side's
    // report should not wait for a batch to fill.
    let mut passed = Instant::now();
    // A damaged line (bytes that are not text) is just a lost frame.
    for line in BufRead::split(reader, b'\n') {
        let line = String::from_utf8_lossy(&line?).trim().to_string();
        if !line.is_empty() {
            batch.push(line);
        }
        if batch.len() >= 32 || (!batch.is_empty() && passed.elapsed() >= Duration::from_millis(50))
        {
            passed = Instant::now();
            if tx.send(std::mem::take(&mut batch)).is_err() {
                return Ok(());
            }
        }
    }
    let _ = tx.send(batch);
    Ok(())
}

/// Makes a one-time key for the transfer to come, keeps its secret half and
/// shows the other: as text, and (on a terminal) as codes for the sender's
/// camera.
fn show_one_time_key() -> Result<()> {
    use std::io::{BufRead, IsTerminal};
    let Some(me) = identity::load()? else {
        bail!(
            "a one-time key goes with a device ID, and this device has none yet (run `qrsend id`)"
        );
    };
    let (secret, key) = me.new_once()?;
    crate::once::keep(&secret)?;
    eprintln!(
        "One-time key for the sender (forward secrecy; destroyed when the transfer has arrived):"
    );
    eprintln!("{}", key.to_text());
    eprintln!(
        "Give it to the sender: `qrsend send … --one-time-key <the line above>` (or \
         `--one-time-key camera`), or the web app's Send page."
    );
    if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
        return Ok(());
    }
    eprint!("Press Enter to show it as codes for the sender's camera (q: the text is enough): ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if answer.trim().eq_ignore_ascii_case("q") {
        return Ok(());
    }
    let codes = key
        .codes(crate::devices::ID_CODES.capacity_chars())?
        .iter()
        .map(|c| qrsend_core::qr::render(c, crate::devices::ID_CODES))
        .collect::<Result<Vec<_>, _>>()?;
    crate::devices::show_codes("One-time key", &codes)
}

pub fn run(args: RecvArgs) -> Result<()> {
    let secure = args.secure;
    if secure {
        // Each of these would open a way out, or leave something behind.
        let against = [
            (
                args.feedback_to.is_some(),
                "--feedback-to",
                "it tells the sender nothing but its one-time key",
            ),
            (
                args.two_way,
                "--two-way",
                "it tells the sender nothing but its one-time key",
            ),
            (
                args.keep,
                "--keep",
                "it keeps nothing in the inbox once it is saved",
            ),
            (
                args.copy,
                "--copy",
                "the clipboard is open to every program",
            ),
            (args.log.is_some(), "--log", "it keeps no log"),
        ];
        if let Some((_, option, why)) = against.iter().find(|(given, ..)| *given) {
            bail!("--secure and {option} do not go together: {why}");
        }
    } else {
        log::start(args.log.as_deref(), "recv")?;
    }
    // (Nothing is taken from the network that way, nor asked of it.)
    let no_lan = args.no_lan || secure;
    // Forward secrecy starts here: the sender has to be shown the key
    // before it packs. The strictest way takes nothing without it.
    if args.one_time_key || secure {
        show_one_time_key()?;
    }
    // Refuses a transfer that `--secure` does not take, and forgets
    // what was stored of it.
    let refuse = |why: &str, store: Option<&Store>| -> anyhow::Error {
        if let Some(store) = store {
            let _ = std::fs::remove_dir_all(&store.dir);
        }
        anyhow::anyhow!(
            "this transfer is refused: {why}, and --secure takes only transfers encrypted \
             for this device and signed by a trusted one"
        )
    };
    if args.camera_fps.is_some_and(|f| !(f.is_finite() && f > 0.0)) {
        bail!("--camera-fps must be a positive number");
    }
    let input = if args.text.is_some() {
        None
    } else if let Some(dev) = &args.camera {
        let dev = if dev.is_empty() {
            crate::input::default_camera().to_string()
        } else {
            dev.clone()
        };
        eprintln!(
            "Scanning camera {dev:?} — point it at the sender's screen (Ctrl-C to stop; progress is saved)."
        );
        Some(Input::Camera {
            device: dev,
            size: args.camera_size,
            fps: args.camera_fps,
        })
    } else if let Some(v) = &args.video {
        Some(Input::Video(v.clone()))
    } else if !args.images.is_empty() {
        Some(Input::Images(image_paths(&args.images)?))
    } else {
        bail!("choose an input: --camera, --video FILE, --images PATH or --text FILE");
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

    log::line("input", || {
        let from = if args.camera.is_some() {
            "camera"
        } else if args.video.is_some() {
            "video"
        } else if !args.images.is_empty() {
            "images"
        } else {
            "text"
        };
        format!(
            "from={from} askedSize={} askedFps={} threads={threads} lan={}",
            args.camera_size
                .map_or("default".into(), |(w, h)| format!("{w}x{h}")),
            args.camera_fps.map_or("default".into(), |f| f.to_string()),
            !no_lan
        )
    });
    let (ftx, frx) = bounded::<Picture>(threads * 2);
    let (ttx, trx) = unbounded::<Vec<String>>();
    let producer = match (input, args.text.clone()) {
        (Some(input), _) => thread::spawn(move || input.produce(ftx)),
        (None, Some(path)) => {
            drop(ftx);
            let ttx = ttx.clone();
            thread::spawn(move || read_text(&path, ttx))
        }
        (None, None) => unreachable!("an input was chosen above"),
    };
    let lattice = Arc::new(std::sync::Mutex::new(decode::Lattice::default()));
    // The size of the pictures being read, once the first one is in.
    let picture = Arc::new(std::sync::OnceLock::<(usize, usize)>::new());
    let colors = Arc::new(decode::ColorWatch::default());
    // Pictures decoded so far, and the time that took (microseconds, all
    // threads together).
    let decoded = Arc::new((
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    ));
    for _ in 0..threads {
        let (frx, ttx, stop, lattice) = (frx.clone(), ttx.clone(), stop.clone(), lattice.clone());
        let (picture, colors, decoded) = (picture.clone(), colors.clone(), decoded.clone());
        thread::spawn(move || {
            while let Ok(f) = frx.recv() {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let _ = picture.set((f.luma.width, f.luma.height));
                let began = Instant::now();
                let texts = decode::detect_picture(&f, &lattice, &colors);
                decoded.0.fetch_add(1, Ordering::Relaxed);
                decoded
                    .1
                    .fetch_add(began.elapsed().as_micros() as u64, Ordering::Relaxed);
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

    // A sender may offer a network connection (`qrsend send --lan`). What
    // arrives through it joins what the codes bring: frames are frames.
    let mut offers = link::Assembler::default();
    let mut link: Option<net::Link> = None;
    // The offer last taken up, and when: a connection that could not be
    // made (a firewall still asking, say) is tried again after a while.
    let mut tried_offer: Option<(u8, Instant)> = None;
    const TRY_AGAIN: Duration = Duration::from_secs(5);
    // Not forever, though: the two may simply not be on one network.
    const TRIES: u32 = 3;
    let mut tries = 0;
    let mut said_size = args.camera.is_none();
    let mut said_foreign_offer = false;
    // For the log: codes read (frames and others), and what was last logged.
    let (mut codes_read, mut offers_read) = (0u64, 0u64);
    let mut logged = (Instant::now(), 0u64, 0u64);
    // For the sender of a two-way transfer: pictures read a second and the
    // size of a dot in them, looked at once a second.
    let mut camera_seen = (Instant::now(), 0u64);
    // When a browser's usual offer was first read.
    let mut usual_offer: Option<Instant> = None;
    let mut said_color = false;
    let mut link_taken = 0u64;
    let mut link_reported = 0u64;
    let mut last_report = Instant::now();
    let mut codes_open = true;
    // The way back to a sender that reads feedback as text.
    let mut way_back: Option<Box<dyn std::io::Write>> = match &args.feedback_to {
        None => None,
        Some(path) if path.as_os_str() == "-" => Some(Box::new(std::io::stdout())),
        Some(path) => Some(Box::new(std::fs::File::create(path).map_err(|e| {
            anyhow::anyhow!("cannot write feedback to {}: {e}", path.display())
        })?)),
    };
    // What was last said that way, and when: said again once a second while
    // nothing changes, so that the sender does not take silence for a
    // receiver that is gone.
    let mut said_back: (Option<Instant>, usize, u64) = (None, 0, 0);
    // And for a sender that reads it with a camera: a window with the code.
    #[cfg(feature = "window")]
    let mut answer = match args.two_way {
        true => {
            eprintln!(
                "Two-way: a window shows what is still missing; let the sender's camera see it."
            );
            Some(display::answer::Answer::open()?)
        }
        false => None,
    };
    #[cfg(not(feature = "window"))]
    if args.two_way {
        bail!(
            "this build has no window to show feedback codes in; over a channel that carries \
             bytes, --feedback-to does the same"
        );
    }
    let (no_codes, no_messages) = (never::<Vec<String>>(), never::<Vec<u8>>());
    let report_due = tick(net::REPLY_EVERY);

    'outer: loop {
        let mut frames = Vec::new();
        let mut offered = Vec::new();
        select! {
            recv(if codes_open { &trx } else { &no_codes }) -> texts => match texts {
                Ok(texts) => {
                    scanned += 1;
                    codes_read += texts.len() as u64;
                    for text in texts {
                        if text.starts_with(link::PREFIX) {
                            offers_read += 1;
                            offered.push(text);
                        } else if text.starts_with(qrsend_core::feedback::NOTICE_PREFIX) {
                            // The sender says in which colors its codes are
                            // stacked: more than one, and every picture is
                            // read color by color, whatever it looks like.
                            if let Ok(notice) = qrsend_core::feedback::SenderNotice::decode(&text)
                                && rx.params().is_none_or(|p| p.session_id == notice.session_id)
                            {
                                colors.told(notice.colors.count_ones() >= 2);
                            }
                        } else if let Ok(frame) = Frame::from_qr_text(&text) {
                            frames.push(frame);
                        }
                    }
                }
                // The input is used up; a connection may still be delivering.
                Err(_) => {
                    codes_open = false;
                    if link.is_none() {
                        break 'outer;
                    }
                }
            },
            recv(link.as_ref().map_or(&no_messages, |l| &l.messages)) -> message => match message {
                Ok(message) => {
                    if link_taken == 0 {
                        say(&pb, "Receiving over the network.");
                        log::line("link", || "first data over the connection".into());
                    }
                    for record in direct::unpack(&message) {
                        link_taken += 1;
                        if let Record::Frame(frame) = Record::parse(record) {
                            frames.push(frame);
                        }
                    }
                }
                Err(_) => {
                    log::line("link", || format!("ended taken={link_taken}"));
                    say(
                        &pb,
                        if link_taken > 0 {
                            "The network connection ended; reading the codes."
                        } else {
                            "No network connection could be made; reading the codes."
                        },
                    );
                    link = None;
                    if !codes_open {
                        break 'outer;
                    }
                }
            },
            recv(report_due) -> _ => {}
        }
        for frame in frames {
            for event in rx.push(frame) {
                match event {
                    Event::Locked(p) => {
                        log::line("rx", || {
                            format!(
                                "transfer found segments={} segmentBytes={} encrypted={}",
                                p.seg_count + 1,
                                1u64 << p.seg_shift,
                                p.flags & qrsend_core::frame::FLAG_ENCRYPTED != 0
                            )
                        });
                        if secure && let Some(why) = refused_as_insecure(&p, None, None) {
                            pb.finish_and_clear();
                            return Err(refuse(why, None));
                        }
                        let s = Session::open_as(Store::open_or_create(p)?, secure)?;
                        if secure
                            && s.manifest.is_some()
                            && let Some(why) =
                                refused_as_insecure(&p, Some(&s.sender), protection(&s.store))
                        {
                            pb.finish_and_clear();
                            return Err(refuse(why, Some(&s.store)));
                        }
                        for i in s.store.done_indices() {
                            rx.mark_done(i);
                        }
                        let resumed = !s.store.done_indices().is_empty();
                        if let Some(m) = &s.manifest {
                            print_manifest_header(&pb, m, &p, &s.sender, protection(&s.store));
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
                            match s.on_meta(&data)? {
                                Ok(bad) => {
                                    for bad in bad {
                                        rx.reset(bad);
                                    }
                                    let m = s.manifest.as_ref().unwrap();
                                    print_manifest_header(
                                        &pb,
                                        m,
                                        &s.store.params(),
                                        &s.sender,
                                        protection(&s.store),
                                    );
                                    if secure
                                        && let Some(why) = refused_as_insecure(
                                            &s.store.params(),
                                            Some(&s.sender),
                                            protection(&s.store),
                                        )
                                    {
                                        pb.finish_and_clear();
                                        return Err(refuse(why, Some(&s.store)));
                                    }
                                }
                                Err(e) => {
                                    rx.reset(0);
                                    log::line("rx", || "file list unusable".into());
                                    say(
                                        &pb,
                                        format!(
                                            "The file list failed verification ({}); waiting for it again",
                                            util::printable(&format!("{e:#}"))
                                        ),
                                    );
                                }
                            }
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
        if camera_seen.0.elapsed() >= Duration::from_secs(1) {
            let (at, then) = camera_seen;
            let pictures = decoded.0.load(Ordering::Relaxed);
            camera_seen = (Instant::now(), pictures);
            let reads = (pictures - then) as f64 / at.elapsed().as_secs_f64();
            rx.set_camera(qrsend_core::feedback::Camera {
                reads_tenths: (reads * 10.0) as u32,
                dot_tenths: (lattice.lock().unwrap().dot_px() * 10.0) as u32,
                colors: 0,
            });
        }
        if !said_color && colors.in_color() {
            said_color = true;
            log::line("camera", || "color codes noticed".into());
            say(&pb, "Color codes: reading red, green and blue apart.");
        }
        if log::on() {
            log::every("progress", Duration::from_secs(2), "rx", || {
                let (at, pictures_then, codes_then) = logged;
                let secs = at.elapsed().as_secs_f64().max(0.001);
                let pictures = decoded.0.load(Ordering::Relaxed);
                let ms = decoded.1.load(Ordering::Relaxed) as f64 / 1000.0;
                logged = (Instant::now(), pictures, codes_read);
                let size = picture
                    .get()
                    .map_or("?".into(), |(w, h)| format!("{w}x{h}"));
                format!(
                    "progress picture={size} pictures={pictures} picturesPerS={:.1} msPerPicture={:.1} codes={codes_read} \
                     codesPerS={:.1} color={} offers={offers_read} segments={}/{} link={} linkTaken={link_taken}",
                    (pictures - pictures_then) as f64 / secs,
                    if pictures > 0 {
                        ms / pictures as f64
                    } else {
                        0.0
                    },
                    (codes_read - codes_then) as f64 / secs,
                    colors.in_color(),
                    rx.completed_count(),
                    rx.segment_total(),
                    if link.is_some() { "open" } else { "none" },
                )
            });
        }
        if !said_size && let Some(&(w, h)) = picture.get() {
            said_size = true;
            let small = w * h < 1280 * 720 && args.camera_size.is_none();
            say(
                &pb,
                format!(
                    "The camera gives {w}×{h} pictures.{}",
                    if small {
                        " Dense codes need more: try --camera-size 1280x720."
                    } else {
                        ""
                    }
                ),
            );
        }
        for text in offered {
            let Some(message) = offers.add(&text) else {
                continue;
            };
            let ours = rx
                .params()
                .is_some_and(|p| p.session_id == message.session_id);
            // A browser's usual offer wants an answer shown to its camera,
            // which this program does not do: if that is all the sender
            // offers, say so once instead of leaving the person to wonder
            // why the network is not used. (A newer web app also makes an
            // offer that needs no answer, taken up below.)
            log::every("offer", Duration::from_secs(5), "link", || {
                format!("offer read kind={} ours={ours}", message.kind)
            });
            if message.kind == link::KIND_OFFER {
                usual_offer.get_or_insert_with(Instant::now);
                continue;
            }
            let seeded = cfg!(feature = "webrtc") && message.kind == link::KIND_SEEDED_OFFER;
            if !(message.kind == link::KIND_TCP_OFFER || seeded)
                || no_lan
                || !ours
                || link.is_some()
                || tries >= TRIES
                || tried_offer.is_some_and(|(id, at)| id == message.id && at.elapsed() < TRY_AGAIN)
            {
                continue;
            }
            if seeded {
                said_foreign_offer = true;
                tried_offer = Some((message.id, Instant::now()));
                tries += 1;
                link_taken = 0;
                link_reported = 0;
                log::line("link", || format!("connecting to a browser try={tries}"));
                say(
                    &pb,
                    "The sender (a browser) offers a network connection; connecting…",
                );
                #[cfg(feature = "webrtc")]
                {
                    link = Some(rtc::connect(message.payload));
                }
            } else if let Ok(offer) = TcpOffer::from_bytes(&message.payload) {
                tried_offer = Some((message.id, Instant::now()));
                tries += 1;
                link_taken = 0;
                link_reported = 0;
                log::line("link", || {
                    let kinds: Vec<&str> = offer
                        .addresses
                        .iter()
                        .map(|a| a.parse().map_or("name", net::address_kind))
                        .collect();
                    format!(
                        "connecting by TCP try={tries} addresses={}",
                        kinds.join(",")
                    )
                });
                say(
                    &pb,
                    format!(
                        "The sender offers a network connection ({}); connecting…",
                        util::printable(&offer.addresses.join(", "))
                    ),
                );
                link = Some(net::connect(offer, message.session_id));
            }
        }
        if !said_foreign_offer
            && usual_offer.is_some_and(|at| at.elapsed() > Duration::from_secs(6))
        {
            said_foreign_offer = true;
            say(
                &pb,
                "The sender (a browser) offers a network connection, but only to another browser. \
                 Reading the codes only.",
            );
        }
        // The sender goes by these reports: what has been taken in, and
        // what is still missing.
        if let Some(l) = &link
            && link_taken != link_reported
            && last_report.elapsed() >= net::REPLY_EVERY
            && let Some(feedback) = rx.feedback(false)
        {
            link_reported = link_taken;
            last_report = Instant::now();
            let _ = l
                .replies
                .send(format!("A{link_taken}\n{}", feedback.encode()));
        }
        // The same report for a sender that reads it as text: when it
        // changes (ten times a second at most), and once a second anyway.
        if let Some(out) = &mut way_back {
            let (at, done, left) = said_back;
            let now = (
                rx.completed_count(),
                rx.progress().map_or(0, |p| p.remaining_symbols),
            );
            let since = at.map(|at| at.elapsed());
            let changed = now != (done, left);
            if since
                .is_none_or(|s| s >= Duration::from_secs(1) || (changed && s >= net::REPLY_EVERY))
                && let Some(feedback) = rx.feedback(false)
            {
                said_back = (Some(Instant::now()), now.0, now.1);
                // (A sender that went away is no reason to stop receiving.)
                if writeln!(out, "{}", feedback.encode())
                    .and_then(|()| out.flush())
                    .is_err()
                {
                    way_back = None;
                }
            }
        }
        #[cfg(feature = "window")]
        if let Some(window) = &mut answer {
            let code = match window.due() {
                true => rx.feedback(false).map(|f| f.encode()),
                false => None,
            };
            if !window.show(code) {
                answer = None;
                log::line("rx", || "feedback window closed".into());
                say(
                    &pb,
                    "The feedback window was closed: receiving one-way from here on.",
                );
            }
            // (Quitting the window's application ends the run as Ctrl-C does.)
            if display::stop_asked() {
                stop.store(true, Ordering::SeqCst);
            }
        }
        if rx.is_complete() || stop.load(Ordering::Relaxed) {
            break 'outer;
        }
    }
    stop.store(true, Ordering::SeqCst);
    pb.finish_and_clear();
    // A short transfer can be over before the loop above comes round to it.
    if !said_color && colors.in_color() {
        eprintln!("Color codes: reading red, green and blue apart.");
    }

    let Some(mut s) = session else {
        // Surface input errors (unreadable file, missing ffmpeg) first.
        if producer.is_finished() {
            producer.join().expect("input thread panicked")?;
        }
        bail!("no QRSend frames found in {scanned} image(s)");
    };
    s.store.save()?;
    let id = s.store.state.session_id;
    log::line("rx", || {
        format!(
            "input over complete={} segments={}/{} pictures={scanned} codes={codes_read} linkTaken={link_taken}",
            s.store.is_complete(),
            rx.completed_count(),
            rx.segment_total()
        )
    });
    if !s.store.is_complete() {
        eprintln!(
            "Received {}/{} segments.",
            rx.completed_count(),
            rx.segment_total()
        );
        if secure {
            // Nothing is left behind, an unfinished transfer no more than a
            // finished one. (It could be finished only for as long as the
            // sender still has what it packed, which a sender that keeps to
            // the same rule has not once it ends.)
            let _ = std::fs::remove_dir_all(&s.store.dir);
            eprintln!(
                "Not kept (--secure): what arrived of this transfer is removed. To receive it, \
                 start again while the sender is showing it."
            );
        } else {
            eprintln!("{}", resume_hint(id, &s.store.missing()));
        }
        log::line("end", || "not complete: kept in the inbox".into());
        std::process::exit(2);
    }
    // Everything is verified and stored: a sender may stop.
    if let Some(out) = &mut way_back
        && let Some(feedback) = rx.feedback(true)
    {
        let _ = writeln!(out, "{}", feedback.encode()).and_then(|()| out.flush());
    }
    if let Some(l) = link.take() {
        if let Some(feedback) = rx.feedback(true) {
            let _ = l
                .replies
                .send(format!("A{link_taken}\n{}", feedback.encode()));
        }
        l.finish();
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
            // Saved: the sender's camera gets a few seconds to read that
            // everything arrived, so that it stops by itself.
            #[cfg(feature = "window")]
            if let Some(window) = &mut answer
                && let Some(feedback) = rx.feedback(true)
            {
                eprintln!("Everything arrived: showing the sender so for a moment…");
                window.hold(feedback.encode(), Duration::from_secs(4));
            }
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
                // Saved, and nothing of it kept: the one-time key it was
                // encrypted to is of no use any more, and from here on
                // nothing on this machine opens the transfer as it was sent.
                let used = s
                    .store
                    .meta_bytes()
                    .map(|meta| crypto::once_used(&meta))
                    .unwrap_or_default();
                s.store.remove()?;
                if !used.is_empty() {
                    crate::once::destroy(&used);
                    log::line("rx", || "one-time key destroyed".into());
                    eprintln!("The one-time key of this transfer is destroyed.");
                }
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
