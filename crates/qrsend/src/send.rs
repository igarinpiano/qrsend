//! `qrsend send`: pack inputs into a spool and stream it as QR codes.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use qrsend_core::frame::{self, MIN_SYMBOL_SIZE};
use qrsend_core::manifest::session_hex;
use qrsend_core::payload::PackOptions;
use qrsend_core::qr::{self, Density, Ec, QrParams};
use qrsend_core::resume::ResumeCode;
use qrsend_core::schedule::ScheduleConfig;
use qrsend_core::sender::{Sender, SessionLayout};
use qrsend_core::{base45, link};

use crate::display::export::Canvas;
use crate::display::{self, FrameStream, GridSpec};
use crate::input::Input;
#[cfg(feature = "webrtc")]
use crate::rtc;
use crate::spool::{Content, Spool, SpoolOptions};
use crate::{collect, firewall, identity, log, net, twoway, util};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DisplayKind {
    /// A native window (largest codes, best throughput)
    Window,
    /// Half-block characters in this terminal
    Terminal,
}

#[derive(clap::Args)]
pub struct SendArgs {
    /// Files or folders to send (`-` reads standard input)
    #[arg(value_name = "PATH")]
    pub paths: Vec<PathBuf>,
    /// The strictest way to send: only encrypted, for trusted devices (--to),
    /// and signed; one way only (no network connection is offered, and no
    /// feedback from the receiver is read); and nothing is left on this
    /// machine afterwards (neither the packed transfer nor a log). Options
    /// that go against any of this are errors. Setting QRSEND_SECURE=1 in
    /// the environment makes this the rule
    #[arg(long, env = "QRSEND_SECURE", value_parser = clap::builder::FalseyValueParser::new())]
    pub secure: bool,
    /// Send this text instead of files
    #[arg(long, conflicts_with = "paths")]
    pub text: Option<String>,
    /// File name to use for standard input
    #[arg(long, default_value = "stdin", value_name = "NAME")]
    pub name: String,
    /// Where to show the codes
    #[arg(long, value_enum, default_value_t = default_display())]
    pub display: DisplayKind,
    /// Write PNG frames to this directory instead of displaying them
    #[arg(long, value_name = "DIR", conflicts_with = "export_video")]
    pub export_frames: Option<PathBuf>,
    /// Write a video instead of displaying: .y4m (uncompressed, no extra tools)
    /// or .mp4/.mkv/.mov/.webm through ffmpeg. Play it anywhere and record or
    /// capture the screen; read it back with `qrsend recv --video`
    #[arg(long, value_name = "FILE", alias = "export-y4m")]
    pub export_video: Option<PathBuf>,
    /// Write the frames as text, one per line, instead of displaying them
    /// ("-" = standard output). For any channel that carries bytes: a serial
    /// line, a TCP connection, ssh, a file. Read it with `qrsend recv --text`.
    /// Frames are 1024 bytes of data each unless a QR size is given
    #[arg(long, value_name = "FILE", conflicts_with_all = ["export_frames", "export_video", "dense"])]
    pub export_text: Option<PathBuf>,
    /// Maximum throughput for exports: fill the frame with as many codes as
    /// fit (same as --grid auto --size 1920x1080 --scale 2, and 30 fps for video)
    #[arg(long)]
    pub dense: bool,
    /// Frame size in pixels when exporting, e.g. 1920x1080 or 3840x2160
    #[arg(long, value_name = "WxH", value_parser = parse_size)]
    pub size: Option<(usize, usize)>,
    /// Number of frames to export (default: --passes full passes)
    #[arg(long, value_name = "N")]
    pub frames: Option<u64>,
    /// Passes to export (default: 1); later passes carry fresh repair
    /// codes, which helps when the playback is recorded with losses
    #[arg(long, value_name = "N")]
    pub passes: Option<f64>,
    /// Pixels per QR module when exporting, and the smallest module size an
    /// automatic grid uses in a window (default: 4, or 2 with --dense)
    #[arg(long, value_name = "PX", value_parser = clap::value_parser!(u16).range(1..=64))]
    pub scale: Option<u16>,
    /// Video quality for ffmpeg (CRF: 0 is lossless, higher is smaller)
    #[arg(long, default_value_t = 12, value_name = "CRF", value_parser = clap::value_parser!(u8).range(0..=51))]
    pub video_crf: u8,
    /// QR density: auto (small codes for small transfers; the best fit for
    /// dense exports), low, normal, high or max
    #[arg(long, value_parser = parse_density, default_value = "auto")]
    pub density: DensityArg,
    /// Explicit QR version (1-40), overrides --density
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=40))]
    pub qr_version: Option<u8>,
    /// Explicit error correction level (L, M, Q, H), overrides --density
    #[arg(long, value_parser = parse_ec)]
    pub ecc: Option<Ec>,
    /// Frames per second (default: 10; 30 for --dense video)
    #[arg(long)]
    pub fps: Option<f64>,
    /// Codes shown at once: N (N×N), COLSxROWS, or auto to fill the window or
    /// frame (window and export only)
    #[arg(long, default_value = "1", value_name = "GRID")]
    pub grid: GridSpec,
    /// Extra repair symbols per pass (fraction of each segment)
    #[arg(long, default_value_t = 0.10)]
    pub redundancy: f64,
    /// zstd level (1-22)
    #[arg(long, default_value_t = 19, value_parser = clap::value_parser!(i32).range(1..=22))]
    pub level: i32,
    /// Do not compress
    #[arg(long)]
    pub no_compress: bool,
    /// Resend only the segments listed in a receiver's resume code
    #[arg(long, value_name = "CODE", conflicts_with_all = ["paths", "text", "session"])]
    pub resume: Option<String>,
    /// Resend a previous session (by id) from the local cache
    #[arg(long, value_name = "ID", conflicts_with_all = ["paths", "text"])]
    pub session: Option<String>,
    /// Do not keep the packed transfer once sending ends. It is kept two
    /// weeks otherwise, to be sent again without packing again (--session,
    /// --resume), and takes about the size of what was sent, compressed.
    /// Without it a transfer that stopped halfway starts over. Setting
    /// QRSEND_NO_KEEP=1 in the environment makes this the rule
    #[arg(long, env = "QRSEND_NO_KEEP", value_parser = clap::builder::FalseyValueParser::new())]
    pub no_keep: bool,
    /// Encrypt for this trusted device (name, fingerprint or qrsend-id:…); repeatable
    #[arg(long, value_name = "DEVICE", conflicts_with = "plain")]
    pub to: Vec<String>,
    /// Send unencrypted: anyone who can see the codes can read the data
    #[arg(long)]
    pub plain: bool,
    /// Name shown to the receiver (defaults to this device's name)
    #[arg(long)]
    pub sender_name: Option<String>,
    /// Also offer a direct connection over the local network: the stream
    /// then carries a code telling a receiving `qrsend` where to connect, and
    /// once it does the transfer travels through the connection as well
    /// (encrypted with a key from that code), far faster than through a
    /// camera. Sending ends by itself when the receiver has everything
    #[arg(long)]
    pub lan: bool,
    /// Address to announce for --lan instead of the ones found (for a machine
    /// on several networks)
    #[arg(long, value_name = "ADDRESS", requires = "lan")]
    pub lan_address: Option<String>,
    /// Port to listen on for --lan (default: any free one)
    #[arg(long, value_name = "PORT", default_value_t = 0, requires = "lan")]
    pub lan_port: u16,
    /// Two-way transfer: read the receiver's feedback codes with a camera
    /// (through ffmpeg) pointed at its screen, send only what it still
    /// lacks, and end when it has everything. The web app's receiver shows
    /// such codes to a sender that asks for them, which this makes the
    /// stream do. Without an answer the transfer goes on one-way
    #[arg(long, conflicts_with = "feedback_from")]
    pub two_way: bool,
    /// The camera for --two-way, as for `qrsend recv --camera`: index on
    /// macOS, /dev/videoN on Linux, the device name on Windows
    #[arg(long, value_name = "DEVICE", requires = "two_way")]
    pub two_way_camera: Option<String>,
    /// Automatic speed: with a receiver that answers (--two-way or
    /// --feedback-from), let its reports choose how many pictures are shown
    /// a second and how many codes a picture holds. One change is tried at
    /// a time, kept if more gets through and taken back if not; it starts
    /// from --fps and one code, and goes up to what fits the window at
    /// --scale. In a window only. --grid COLSxROWS keeps the layout and
    /// tunes the speed alone
    #[arg(long)]
    pub auto_speed: bool,
    /// Color codes: three codes in every place, one each in red, green and
    /// blue, for three times the data per picture with a camera that tells
    /// the colors apart (a camera that does not reads one of the three, or
    /// none: with a receiver that answers, the colors it cannot use are
    /// given up along the way). In a window and with --export-frames
    #[arg(long)]
    pub color: bool,
    /// Two-way transfer over whatever carries bytes: read the receiver's
    /// feedback codes as text, one per line, from FILE ("-": standard
    /// input), as `qrsend recv --feedback-to` writes them. With
    /// --export-text, frames are then written until the receiver has
    /// everything (unless --frames or --passes says how many)
    #[arg(long, value_name = "FILE")]
    pub feedback_from: Option<PathBuf>,
    /// Write the diagnostic log to FILE ("-": standard error) instead of
    /// keeping it for `qrsend log`. It holds no file names, contents, device
    /// IDs or network addresses
    #[arg(long, value_name = "FILE")]
    pub log: Option<PathBuf>,
    /// log2 of the segment size in bytes (advanced)
    #[arg(long, default_value_t = 20, hide = true, value_parser = clap::value_parser!(u8).range(12..=30))]
    pub seg_shift: u8,
}

fn default_display() -> DisplayKind {
    if cfg!(feature = "window") {
        DisplayKind::Window
    } else {
        DisplayKind::Terminal
    }
}

/// `--density`: a preset, or automatic selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DensityArg {
    Auto,
    Preset(Density),
}

fn parse_density(s: &str) -> Result<DensityArg, String> {
    if s == "auto" {
        return Ok(DensityArg::Auto);
    }
    s.parse()
        .map(DensityArg::Preset)
        .map_err(|_| format!("unknown density {s:?} (use auto, low, normal, high or max)"))
}

pub(crate) fn parse_size(s: &str) -> Result<(usize, usize), String> {
    let bad = || format!("invalid size {s:?} (use WIDTHxHEIGHT, e.g. 1920x1080)");
    let (w, h) = s.split_once(['x', 'X', '×']).ok_or_else(bad)?;
    let (w, h): (usize, usize) = (w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
    if !(64..=16384).contains(&w) || !(64..=16384).contains(&h) {
        return Err(bad());
    }
    Ok((w, h))
}

fn parse_ec(s: &str) -> Result<Ec, String> {
    s.parse()
}

/// Data bytes per frame for `--export-text` (a line of about 1600 characters).
const TEXT_SYMBOL_SIZE: usize = 1024;

pub fn run(args: SendArgs) -> Result<()> {
    let secure = args.secure;
    if secure {
        // Each of these would open a way in, or leave something behind.
        let against = [
            (
                args.plain,
                "--plain",
                "it sends encrypted transfers only: name the receiver with --to <device>",
            ),
            (args.lan, "--lan", "it sends nothing through the network"),
            (
                args.two_way || args.feedback_from.is_some(),
                "--two-way / --feedback-from",
                "it takes nothing in from the receiver",
            ),
            (args.log.is_some(), "--log", "it keeps no log"),
        ];
        if let Some((_, option, why)) = against.iter().find(|(given, ..)| *given) {
            bail!("--secure and {option} do not go together: {why}");
        }
    } else {
        log::start(args.log.as_deref(), "send")?;
    }
    let lan = args.lan;
    let no_keep = args.no_keep || secure;
    let exporting = args.export_frames.is_some() || args.export_video.is_some();
    let in_window = !exporting
        && args.export_text.is_none()
        && cfg!(feature = "window")
        && args.display == DisplayKind::Window;
    if args.color && !in_window && args.export_frames.is_none() {
        bail!("--color applies to a window and to --export-frames");
    }
    if args.auto_speed && !in_window {
        bail!("--auto-speed applies to a window (it changes what the window shows as it goes)");
    }
    if args.auto_speed && !args.two_way && args.feedback_from.is_none() {
        bail!(
            "--auto-speed goes by what the receiver reports: add --two-way (a camera reads its \
             feedback codes) or --feedback-from"
        );
    }
    let fps = args
        .fps
        .unwrap_or(if args.dense && args.export_video.is_some() {
            30.0
        } else {
            10.0
        });
    let passes = args.passes.unwrap_or(1.0);
    let positive = |v: f64| v.is_finite() && v > 0.0;
    if !(0.0..=10.0).contains(&args.redundancy) || !positive(fps) || !positive(passes) {
        bail!("--redundancy must be within 0..10, and --fps and --passes positive");
    }
    if args.dense && !exporting {
        bail!("--dense applies to --export-video / --export-frames (in a window, use --grid auto)");
    }
    let grid = if args.dense && args.grid == GridSpec::ONE {
        GridSpec::Auto
    } else {
        args.grid
    };
    let size = args
        .size
        .or((args.dense || (exporting && grid == GridSpec::Auto)).then_some((1920, 1080)));
    let scale = args
        .scale
        .map(usize::from)
        .unwrap_or(if args.dense { 2 } else { 4 });

    let (spool, only) = if let Some(code) = &args.resume {
        let rc = ResumeCode::decode(code)?;
        let body: Vec<u32> = rc.segments.iter().copied().filter(|&i| i != 0).collect();
        (Spool::open(rc.session_id)?, Some(body))
    } else if let Some(id) = &args.session {
        (Spool::open(util::parse_session(id)?)?, None)
    } else {
        if args.to.is_empty() && secure {
            bail!(
                "--secure sends encrypted transfers only: name the receiver with --to <device> \
                 (see `qrsend devices`)"
            );
        }
        // (An ID given in full is taken as it is otherwise; here it has to
        // be one of the devices this one trusts.)
        if secure
            && let Some(unknown) = args
                .to
                .iter()
                .find(|t| t.starts_with("qrsend-id:") && !identity::is_trusted(t))
        {
            bail!(
                "--secure sends to trusted devices only, and {} is not one (see `qrsend devices`)",
                util::printable(unknown)
            );
        }
        if args.to.is_empty() && !args.plain {
            bail!(
                "choose who can read this: --to <device> to encrypt for a trusted device \
                 (see `qrsend devices`), or --plain to send unencrypted"
            );
        }
        let recipients = args
            .to
            .iter()
            .map(|t| identity::resolve(t))
            .collect::<Result<Vec<_>>>()?;
        // Whoever is trusted with its older keys only is encrypted to the
        // older way: worth saying, since one step mends it.
        let older: Vec<String> = recipients
            .iter()
            .filter(|r| r.pq.is_none())
            .map(|r| util::printable(&r.name))
            .collect();
        if secure && !older.is_empty() {
            bail!(
                "--secure sends only what post-quantum keys protect, and {} is trusted with \
                 older keys only: have it show its ID again (`qrsend id`) and add that",
                older.join(", ")
            );
        }
        if !older.is_empty() {
            eprintln!(
                "Note: {} is trusted with keys from before post-quantum ones, so this transfer \
                 is encrypted the older way for it. Have it show its ID again (`qrsend id`) and \
                 add that with `qrsend devices add`.",
                older.join(", ")
            );
        }
        // Sign whenever this device has (or, when encrypting, gets) an identity.
        let signer = if recipients.is_empty() {
            identity::load()?
        } else {
            let (me, created) = identity::load_or_create(None)?;
            if created {
                eprintln!(
                    "Created a device identity for signing ({}).",
                    me.public().fingerprint()
                );
            }
            Some(me)
        };
        let content = if let Some(text) = args.text.clone() {
            Content::Text(text)
        } else if args.paths.is_empty() {
            if std::io::stdin().is_terminal() {
                bail!(
                    "nothing to send: give files/folders, --text, or pipe data into `qrsend send -`"
                );
            }
            let mut text = Vec::new();
            std::io::stdin().read_to_end(&mut text)?;
            match String::from_utf8(text) {
                Ok(t) => Content::Text(t),
                Err(e) => bail!(
                    "standard input is binary ({} bytes); use `qrsend send - --name FILE` to send it as a file",
                    e.into_bytes().len()
                ),
            }
        } else {
            let (items, warnings) = collect::collect(&args.paths, &args.name)?;
            for w in warnings {
                eprintln!("warning: {w}");
            }
            Content::Items(items)
        };
        let workers = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        let pack = PackOptions {
            zstd_level: (!args.no_compress).then_some(args.level),
            zstd_workers: workers,
        };
        eprintln!("Packing…");
        let sender_name = args
            .sender_name
            .clone()
            .or_else(|| signer.as_ref().map(|s| s.name.clone()));
        let opts = SpoolOptions {
            seg_shift: args.seg_shift,
            pack,
            sender_name,
            recipients,
            signer: signer.as_ref(),
        };
        (Spool::create(content, &opts)?, None)
    };
    if secure && spool.info.recipients.is_empty() {
        bail!("--secure sends encrypted transfers only, and this one was packed unencrypted");
    }
    let _discard = no_keep.then(|| Discard::new(&spool));
    interruptible(no_keep.then(|| spool.dir.clone()));

    // Explicit choices win; otherwise pick the code size from the transfer
    // (small transfers) or from the frame (dense exports).
    let params = match (args.qr_version, args.ecc, args.density) {
        (None, None, DensityArg::Auto) => match size {
            Some((w, h)) if grid == GridSpec::Auto => {
                qr::best_tiling(w, h, scale)
                    .with_context(|| {
                        format!("no QR code fits in {w}×{h} at {scale} px per module")
                    })?
                    .params
            }
            _ => qr::auto_params(spool.info.meta_len, spool.info.body_len),
        },
        (version, ec, density) => {
            let preset = match density {
                DensityArg::Preset(d) => d.params(),
                DensityArg::Auto => Density::Normal.params(),
            };
            QrParams {
                version: version.unwrap_or(preset.version),
                ec: ec.unwrap_or(preset.ec),
            }
        }
    };
    // Text frames need not fit a QR code: larger ones cost less per byte.
    let plain_text = args.export_text.is_some()
        && args.qr_version.is_none()
        && args.ecc.is_none()
        && args.density == DensityArg::Auto;
    let symbol_size = if plain_text {
        TEXT_SYMBOL_SIZE
    } else {
        params.symbol_size()
    };
    if symbol_size < MIN_SYMBOL_SIZE {
        bail!(
            "QR version {} with ECC {:?} is too small for QRSend frames",
            params.version,
            params.ec
        );
    }

    let layout = spool.layout(symbol_size)?;
    let config = ScheduleConfig {
        redundancy: args.redundancy,
        ..ScheduleConfig::default()
    };
    let sender = Sender::new(layout, spool.source()?, config, only.as_deref());
    let mut stream = FrameStream::new(sender, params);
    if args.color {
        stream.in_color();
    }
    let per_pass = stream.frames_per_pass();
    log::line("tx", || {
        let shown = if args.export_frames.is_some() {
            "pictures"
        } else if args.export_video.is_some() {
            "video"
        } else if args.export_text.is_some() {
            "text"
        } else {
            "screen"
        };
        format!(
            "packed wireBytes={} fileListBytes={} qr=v{}-{:?} bytesPerCode={symbol_size} codesPerPass={per_pass} \
             shownAs={shown} encrypted={} lan={} resumed={}",
            spool.info.body_len + spool.info.meta_len as u64,
            spool.info.meta_len,
            params.version,
            params.ec,
            !spool.info.recipients.is_empty(),
            lan,
            only.is_some(),
        )
    });
    if lan {
        // The receiver is the one to connect, and a firewall that turns it
        // away tells neither side: better to see to that before the wait
        // (and before the socket it is about is opened).
        if let Some((state, program)) = firewall::incoming() {
            let state = firewall::settle(state, &program);
            log::line("link", || format!("firewall incoming={}", state.word()));
        }
        let (listener, offer) = net::listen(args.lan_address.as_deref(), args.lan_port)?;
        // The offer travels as codes like the frames do, so its parts must
        // be no longer than a frame.
        let room = base45::encoded_len(frame::OVERHEAD + symbol_size);
        let codes = link::split(
            spool.info.session_id,
            link::KIND_TCP_OFFER,
            rand::random(),
            &offer.to_bytes(),
            room,
        )?;
        log::line("link", || {
            let kinds: Vec<&str> = offer
                .addresses
                .iter()
                .map(|a| a.parse().map_or("name", net::address_kind))
                .collect();
            format!("TCP: offering a connection addresses={}", kinds.join(","))
        });
        eprintln!(
            "Offering a network connection at {} port {}.",
            offer.addresses.join(", "),
            offer.port
        );
        let (events_tx, events) = crossbeam_channel::unbounded();
        let wide = SessionLayout {
            symbol_size: net::SYMBOL_SIZE,
            ..layout
        };
        let id = spool.info.session_id;
        net::serve(
            listener,
            offer,
            wide,
            move || Spool::open(id)?.source(),
            events_tx.clone(),
        );
        // And one to a receiving browser, which cannot use the above.
        #[cfg(feature = "webrtc")]
        if let Err(e) = rtc::serve(
            id,
            args.lan_address.as_deref(),
            room,
            layout,
            move || Spool::open(id)?.source(),
            events_tx,
        ) {
            eprintln!("No connection is offered to browsers: {e:#}");
        }
        stream.with_link(codes, events);
    }
    // The way back from the receiver, where there is one.
    let way_back = if let Some(path) = &args.feedback_from {
        Some(twoway::Source::Text(path.clone()))
    } else if args.two_way {
        let device = args
            .two_way_camera
            .clone()
            .unwrap_or_else(|| crate::input::default_camera().to_string());
        eprintln!(
            "Two-way: point camera {device:?} at the receiver's screen; it shows what it still lacks."
        );
        Some(twoway::Source::Pictures(Input::Camera {
            device,
            size: None,
            fps: None,
        }))
    } else {
        None
    };
    let answered = way_back.is_some();
    if let Some(source) = way_back {
        log::line("tx", || {
            format!(
                "two-way by={}",
                if args.two_way { "camera" } else { "text" }
            )
        });
        stream.with_feedback(twoway::listen(source));
    }
    // After an export there is no display loop to end; with --lan the
    // transfer goes on through the connection until the receiver has it all.
    let serve_until_done = |stream: &mut FrameStream| {
        if !lan {
            return;
        }
        eprintln!("Waiting for the receiver on the network (Ctrl-C to stop)…");
        while !stream.finished() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        eprintln!("The receiver has everything.");
    };
    if let Some(path) = &args.export_text {
        // With a receiver that answers, frames go out until it has
        // everything, unless told how many.
        let count = match (args.frames, args.passes) {
            (None, None) if answered => None,
            (frames, _) => {
                Some(frames.unwrap_or((per_pass as f64 * passes).ceil().max(1.0) as u64))
            }
        };
        eprintln!(
            "Session {} · {}",
            session_hex(spool.info.session_id),
            spool.info.summary
        );
        let count = display::export::text(&mut stream, path, count)?;
        if stream.finished() {
            eprintln!("The receiver has everything.");
        } else if answered && !stream.answered() {
            eprintln!("No feedback came from the receiver.");
        }
        eprintln!(
            "Wrote {count} frame(s) of {symbol_size} B as text to {}. Read them with: qrsend recv --text <file or ->",
            if path.as_os_str() == "-" {
                "standard output".to_string()
            } else {
                path.display().to_string()
            }
        );
        serve_until_done(&mut stream);
        return Ok(());
    }
    let canvas = if exporting {
        Some(Canvas::new(params.modules(), grid, size, scale)?)
    } else {
        None
    };
    // (With color codes, three in every place.)
    let layers = if args.color { 3 } else { 1 };
    let per_tick = layers
        * match (&canvas, grid) {
            (Some(c), _) => c.codes(),
            (None, GridSpec::Fixed { cols, rows }) => cols * rows,
            (None, GridSpec::Auto) => 1,
        };
    eprintln!(
        "Session {} · {}",
        session_hex(spool.info.session_id),
        spool.info.summary
    );
    let rate = symbol_size as f64 * fps * per_tick as f64;
    log::line("tx", || {
        format!(
            "showing codesPerPicture={} fps={fps}",
            if grid == GridSpec::Auto && canvas.is_none() {
                "auto".to_string()
            } else {
                per_tick.to_string()
            }
        )
    });
    eprintln!(
        "{} on the wire · QR v{}-{:?} · {} B per code · {} codes per pass{}",
        util::human_bytes(spool.info.body_len + spool.info.meta_len as u64),
        params.version,
        params.ec,
        symbol_size,
        per_pass,
        if grid == GridSpec::Auto && canvas.is_none() {
            String::new()
        } else {
            format!(
                " (≈{} at {fps} fps × {per_tick} = {}/s)",
                util::human_duration(per_pass as f64 / (fps * per_tick as f64)),
                util::human_bytes(rate as u64)
            )
        },
    );
    if spool.info.recipients.is_empty() {
        eprintln!("Unencrypted: anyone who can see the codes can read this transfer.");
    } else {
        eprintln!("Encrypted for: {}", spool.info.recipients.join(", "));
    }
    if let Some(only) = &only {
        eprintln!("Resending {} segment(s) from the resume code.", only.len());
    }

    if let Some(canvas) = canvas {
        let count = args.frames.unwrap_or(
            ((per_pass as f64 * passes) / (canvas.codes() * layers) as f64)
                .ceil()
                .max(1.0) as u64,
        );
        let what = format!(
            "{count} frame(s), {}×{} px, {}×{} codes",
            canvas.width, canvas.height, canvas.cols, canvas.rows
        );
        if let Some(dir) = &args.export_frames {
            display::export::png_frames(&mut stream, dir, count, canvas)?;
            eprintln!("Wrote {what} to {}", dir.display());
        } else if let Some(path) = &args.export_video {
            display::export::video(&mut stream, path, count, canvas, fps, args.video_crf)?;
            eprintln!("Wrote {what} at {fps} fps to {}", path.display());
            eprintln!("Read it back (or a recording of it) with: qrsend recv --video <file>");
        }
        serve_until_done(&mut stream);
    } else {
        match args.display {
            DisplayKind::Terminal => display::terminal::run(&mut stream, fps)?,
            #[cfg(feature = "window")]
            DisplayKind::Window => {
                // (Left to the reports, the layout is theirs to choose
                // unless one was asked for.)
                let grid = match (args.auto_speed, args.grid) {
                    (true, GridSpec::ONE) => GridSpec::Auto,
                    _ => grid,
                };
                display::window::run(&mut stream, fps, grid, scale, args.auto_speed)?
            }
            #[cfg(not(feature = "window"))]
            DisplayKind::Window => {
                bail!("this build has no window support; use --display terminal")
            }
        }
        if stream.finished() {
            eprintln!("The receiver has everything.");
        } else {
            eprintln!(
                "Sent {} codes ({} pass(es)).",
                stream.frames,
                stream.pass() + 1
            );
        }
    }
    if no_keep {
        log::line("tx", || "the packed transfer is not kept".into());
        eprintln!(
            "Not kept ({}): to send this again, send the files again.",
            if args.no_keep {
                "--no-keep"
            } else {
                "--secure"
            }
        );
    } else {
        eprintln!(
            "To resend later: qrsend send --session {}  (or --resume <code> from the receiver)",
            session_hex(spool.info.session_id)
        );
    }
    Ok(())
}

/// Removes a spool when sending ends (`--no-keep`): when this goes out of
/// scope, however `run` is left, and when the program is interrupted. A
/// sender that is killed outright removes nothing: the spool says that it
/// was not to be kept, and the next `send` removes it.
struct Discard(PathBuf);

impl Discard {
    fn new(spool: &Spool) -> Discard {
        spool.mark_not_kept();
        Discard(spool.dir.clone())
    }
}

impl Drop for Discard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Ctrl-C ended a window's loop, and `run` went on to its end from there.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Says what Ctrl-C does from here on. While a window shows the codes, it
/// ends the window's loop, as closing the window does: `run` then says what
/// it has to say and leaves in order. At any other time (packing, an
/// export, the wait for a receiver on the network) the program ends at
/// once, after removing `discard`, the spool that is not to be kept.
fn interruptible(discard: Option<PathBuf>) {
    let _ = ctrlc::set_handler(move || {
        // (Asked a second time, the loop is not waited for.)
        if display::ask_stop() {
            INTERRUPTED.store(true, Ordering::SeqCst);
            return;
        }
        if let Some(dir) = &discard {
            let _ = std::fs::remove_dir_all(dir);
        }
        end_interrupted();
    });
}

/// Ends as a program does that was interrupted (a shell running a script
/// goes by that to stop the script too, and so does the npm launcher): by
/// the signal itself, now that it is handled.
fn end_interrupted() -> ! {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::raise(libc::SIGINT);
    }
    std::process::exit(130);
}

/// Called when everything else is done: a run that Ctrl-C brought to its
/// end still ends as an interrupted one.
pub fn end_if_interrupted() {
    if INTERRUPTED.load(Ordering::SeqCst) {
        end_interrupted();
    }
}
