//! `qrsend inbox`: inspect, export and clean up receive sessions.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use qrsend_core::manifest::{EntryType, session_hex};
use qrsend_core::resume::ResumeCode;

use crate::extract::{self, Conflict, ExtractOptions};
use crate::recv::{Session, report};
use crate::store::Store;
use crate::util;

#[derive(Subcommand)]
pub enum InboxCmd {
    /// List receive sessions
    List,
    /// Show a session's details and file list
    Show { id: String },
    /// Save a completed session's files
    Export {
        id: String,
        #[arg(short, long, default_value = ".", value_name = "DIR")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = Conflict::Rename)]
        on_conflict: Conflict,
        /// Write a single file (or text) to standard output
        #[arg(long)]
        stdout: bool,
        /// Remove the session from the inbox afterwards
        #[arg(long)]
        remove: bool,
    },
    /// Print the resume code listing missing segments
    Missing { id: String },
    /// Delete a session
    Rm { id: String },
}

pub fn run(cmd: InboxCmd) -> Result<()> {
    match cmd {
        InboxCmd::List => {
            let sessions = Store::list();
            if sessions.is_empty() {
                println!("Inbox is empty.");
            }
            for st in sessions {
                let total = st.seg_count as usize + 1;
                let done = util::parse_ranges(&st.done)?.len();
                let status = if done == total && st.unverified.is_empty() {
                    "complete".to_string()
                } else {
                    format!("{done}/{total}")
                };
                println!(
                    "{}  {:>11}  {}",
                    session_hex(st.session_id),
                    status,
                    st.summary
                        .as_deref()
                        .unwrap_or("(file list not received yet)")
                );
            }
        }
        InboxCmd::Show { id } => {
            let s = Session::open(Store::open(util::parse_session(&id)?)?)?;
            let st = &s.store.state;
            println!("Session:  {}", session_hex(st.session_id));
            println!(
                "Segments: {}/{}",
                s.store.done_indices().len(),
                st.seg_count + 1
            );
            match &s.manifest {
                None => println!("Files:    (not received yet)"),
                Some(m) => {
                    println!("Content:  {}", crate::recv::summary(m));
                    for e in m.entries.iter().take(200) {
                        match e.kind {
                            EntryType::Dir => println!("  {}/", e.path),
                            EntryType::File => {
                                println!("  {}  ({})", e.path, util::human_bytes(e.file_size()))
                            }
                        }
                    }
                    if m.entries.len() > 200 {
                        println!("  … and {} more", m.entries.len() - 200);
                    }
                }
            }
        }
        InboxCmd::Export {
            id,
            out,
            on_conflict,
            stdout,
            remove,
        } => {
            let s = Session::open(Store::open(util::parse_session(&id)?)?)?;
            if !s.store.is_complete() {
                bail!("session {id} is not complete yet; see `qrsend inbox missing {id}`");
            }
            let manifest = s.manifest.clone().expect("complete");
            let opts = ExtractOptions {
                out,
                conflict: on_conflict,
                stdout,
                copy: false,
            };
            report(extract::finalize(&s.store, &manifest, &opts)?, stdout);
            if remove {
                s.store.remove()?;
            }
        }
        InboxCmd::Missing { id } => {
            let store = Store::open(util::parse_session(&id)?)?;
            let missing = store.missing();
            if missing.is_empty() {
                println!("Nothing is missing.");
            } else {
                println!(
                    "{}",
                    ResumeCode::new(store.state.session_id, missing).encode()
                );
            }
        }
        InboxCmd::Rm { id } => {
            Store::open(util::parse_session(&id)?)?.remove()?;
            println!("Removed {id}.");
        }
    }
    Ok(())
}
