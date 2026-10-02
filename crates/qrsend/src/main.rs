//! QRSend command-line interface.

mod collect;
mod display;
mod extract;
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
        sufficiently large subset of them."
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
    /// Manage received sessions
    #[command(subcommand)]
    Inbox(inbox::InboxCmd),
    /// Manage cached send sessions
    #[command(subcommand)]
    Cache(CacheCmd),
    /// Print shell completions
    Completions { shell: clap_complete::Shell },
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
        Cmd::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "qrsend", &mut std::io::stdout());
            Ok(())
        }
    }
}
