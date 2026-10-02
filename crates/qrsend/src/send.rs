//! `qrsend send`: pack inputs into a spool and stream it as QR codes.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::ValueEnum;
use qrsend_core::frame::MIN_SYMBOL_SIZE;
use qrsend_core::manifest::session_hex;
use qrsend_core::payload::PackOptions;
use qrsend_core::qr::{Density, Ec, QrParams};
use qrsend_core::resume::ResumeCode;
use qrsend_core::schedule::ScheduleConfig;
use qrsend_core::sender::Sender;

use crate::display::{self, FrameStream};
use crate::spool::{Content, Spool, SpoolOptions};
use crate::{collect, identity, util};

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
    #[arg(long, value_name = "DIR", conflicts_with = "export_y4m")]
    pub export_frames: Option<PathBuf>,
    /// Write a Y4M video instead of displaying (playable, and usable as a fake camera)
    #[arg(long, value_name = "FILE")]
    pub export_y4m: Option<PathBuf>,
    /// Number of frames to export (default: one full pass)
    #[arg(long, value_name = "N")]
    pub frames: Option<u64>,
    /// Pixels per QR module when exporting
    #[arg(long, default_value_t = 4, value_name = "PX")]
    pub scale: usize,
    /// QR density preset
    #[arg(long, value_parser = parse_density, default_value = "normal")]
    pub density: Density,
    /// Explicit QR version (1-40), overrides --density
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=40))]
    pub qr_version: Option<u8>,
    /// Explicit error correction level (L, M, Q, H), overrides --density
    #[arg(long, value_parser = parse_ec)]
    pub ecc: Option<Ec>,
    /// Codes shown per second
    #[arg(long, default_value_t = 10.0)]
    pub fps: f64,
    /// Show N×N codes at once (window and export only)
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=4))]
    pub grid: u8,
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
    /// Encrypt for this trusted device (name, fingerprint or qrsend-id:…); repeatable
    #[arg(long, value_name = "DEVICE", conflicts_with = "plain")]
    pub to: Vec<String>,
    /// Send unencrypted: anyone who can see the codes can read the data
    #[arg(long)]
    pub plain: bool,
    /// Name shown to the receiver (defaults to this device's name)
    #[arg(long)]
    pub sender_name: Option<String>,
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

fn parse_density(s: &str) -> Result<Density, String> {
    s.parse()
}

fn parse_ec(s: &str) -> Result<Ec, String> {
    s.parse()
}

pub fn run(args: SendArgs) -> Result<()> {
    let preset = args.density.params();
    let params = QrParams {
        version: args.qr_version.unwrap_or(preset.version),
        ec: args.ecc.unwrap_or(preset.ec),
    };
    let symbol_size = params.symbol_size();
    if symbol_size < MIN_SYMBOL_SIZE {
        bail!(
            "QR version {} with ECC {:?} is too small for QRSend frames",
            params.version,
            params.ec
        );
    }
    if !(0.0..=10.0).contains(&args.redundancy) || args.fps <= 0.0 {
        bail!("--redundancy must be within 0..10 and --fps positive");
    }

    let (spool, only) = if let Some(code) = &args.resume {
        let rc = ResumeCode::decode(code)?;
        let body: Vec<u32> = rc.segments.iter().copied().filter(|&i| i != 0).collect();
        (Spool::open(rc.session_id)?, Some(body))
    } else if let Some(id) = &args.session {
        (Spool::open(util::parse_session(id)?)?, None)
    } else {
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

    let layout = spool.layout(symbol_size)?;
    let config = ScheduleConfig {
        redundancy: args.redundancy,
        ..ScheduleConfig::default()
    };
    let sender = Sender::new(layout, spool.source()?, config, only.as_deref());
    let mut stream = FrameStream::new(sender, params);
    let grid = args.grid as usize;
    let per_pass = stream.frames_per_pass();
    let seconds = per_pass as f64 / (args.fps * (grid * grid) as f64);
    eprintln!(
        "Session {} · {}",
        session_hex(spool.info.session_id),
        spool.info.summary
    );
    eprintln!(
        "{} on the wire · QR v{}-{:?} · {} B per code · {} codes per pass (≈{} at {} fps{})",
        util::human_bytes(spool.info.body_len + spool.info.meta_len as u64),
        params.version,
        params.ec,
        symbol_size,
        per_pass,
        util::human_duration(seconds),
        args.fps,
        if grid > 1 {
            format!(" × {}", grid * grid)
        } else {
            String::new()
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

    let count = args
        .frames
        .unwrap_or(per_pass.div_ceil((grid * grid) as u64));
    if let Some(dir) = &args.export_frames {
        display::export::png_frames(&mut stream, dir, count, grid, args.scale)?;
        eprintln!("Wrote {count} frame(s) to {}", dir.display());
    } else if let Some(path) = &args.export_y4m {
        display::export::y4m(&mut stream, path, count, grid, args.scale, args.fps)?;
        eprintln!("Wrote {count} frame(s) to {}", path.display());
    } else {
        match args.display {
            DisplayKind::Terminal => display::terminal::run(&mut stream, args.fps)?,
            #[cfg(feature = "window")]
            DisplayKind::Window => display::window::run(&mut stream, args.fps, grid)?,
            #[cfg(not(feature = "window"))]
            DisplayKind::Window => {
                bail!("this build has no window support; use --display terminal")
            }
        }
        eprintln!(
            "Sent {} codes ({} pass(es)).",
            stream.frames,
            stream.pass() + 1
        );
    }
    eprintln!(
        "To resend later: qrsend send --session {}  (or --resume <code> from the receiver)",
        session_hex(spool.info.session_id)
    );
    Ok(())
}
