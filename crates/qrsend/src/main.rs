//! QRSend command-line interface.

mod collect;
mod decode;
mod devices;
mod display;
mod extract;
mod identity;
mod inbox;
mod input;
mod paths;
mod recv;
mod send;
mod spool;
mod store;
mod util;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "qrsend",
    version,
    about = "Send any data — text, files, folders — through a stream of QR codes",
    long_about = "Send any data — text, files, folders — through a stream of QR codes.\n\n\
        No network, no pairing server: the sender shows an endless stream of\n\
        fountain-coded QR codes and the receiver rebuilds the data from any\n\
        sufficiently large subset of them.",
    after_long_help = "QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show files, folders or text as a QR code stream
    Send(send::SendArgs),
    /// Receive a QR code stream from a video or images
    Recv(recv::RecvArgs),
    /// Show this device's ID (for others to encrypt to it)
    Id(devices::IdArgs),
    /// Manage trusted devices
    #[command(subcommand)]
    Devices(devices::DevicesCmd),
    /// Manage received sessions
    #[command(subcommand)]
    Inbox(inbox::InboxCmd),
    /// Manage cached send sessions
    #[command(subcommand)]
    Cache(CacheCmd),
    /// Print shell completions
    Completions { shell: clap_complete::Shell },
    /// Report what each QR detector finds in images (diagnostics)
    #[command(hide = true)]
    DebugDetect {
        images: Vec<std::path::PathBuf>,
        /// Also time each detector on its own (slow on dense frames)
        #[arg(long)]
        each: bool,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    /// List cached send sessions
    List,
    /// Delete cached send sessions
    Clean,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Send(args) => send::run(args),
        Cmd::Recv(args) => recv::run(args),
        Cmd::Id(args) => devices::id(args),
        Cmd::Devices(cmd) => devices::devices(cmd),
        Cmd::Inbox(cmd) => inbox::run(cmd),
        Cmd::Cache(CacheCmd::List) => {
            let spools = spool::Spool::list();
            if spools.is_empty() {
                println!("No cached send sessions.");
            }
            for s in spools {
                println!(
                    "{}  {:>10}  {}",
                    qrsend_core::manifest::session_hex(s.info.session_id),
                    util::human_bytes(s.disk_size()),
                    s.info.summary
                );
            }
            Ok(())
        }
        Cmd::Cache(CacheCmd::Clean) => {
            let spools = spool::Spool::list();
            let n = spools.len();
            for s in spools {
                s.remove()?;
            }
            println!("Removed {n} cached session(s).");
            Ok(())
        }
        Cmd::DebugDetect { images, each } => {
            for path in images {
                let img = image::open(&path)?.into_luma8();
                let f = input::LumaFrame {
                    width: img.width() as usize,
                    height: img.height() as usize,
                    pixels: img.into_raw(),
                };
                let valid = |v: &[String]| {
                    v.iter()
                        .filter(|t| qrsend_core::frame::Frame::from_qr_text(t).is_ok())
                        .count()
                };
                let t = std::time::Instant::now();
                let merged = decode::detect(&f);
                let tm = t.elapsed();
                print!("{}: {} frames in {tm:.0?}", path.display(), valid(&merged));
                if each {
                    let t = std::time::Instant::now();
                    let a = decode::detect_rxing(&f);
                    let ta = t.elapsed();
                    let t = std::time::Instant::now();
                    let b = decode::detect_rqrr(&f);
                    print!(
                        " · rxing alone {} ({ta:.0?}) · rqrr alone {} ({:.0?})",
                        valid(&a),
                        valid(&b),
                        t.elapsed()
                    );
                }
                println!();
            }
            Ok(())
        }
        Cmd::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "qrsend", &mut std::io::stdout());
            Ok(())
        }
    }
}
