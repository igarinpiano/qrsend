//! Turning a completed inbox session into files (or text).

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use qrsend_core::crypto::{self, DeviceIdentity};
use qrsend_core::frame::FLAG_ENCRYPTED;
use qrsend_core::manifest::{Entry, EntryType, Kind, Manifest, session_hex};
use qrsend_core::payload::{UnpackSink, unpack, unpack_text};
use qrsend_core::sanitize::{SafePath, numbered_name};

use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Conflict {
    /// Keep both: save as "name (1).ext"
    Rename,
    /// Replace existing files (directories are merged)
    Overwrite,
    /// Keep the existing file and drop the received one
    Skip,
}

pub struct ExtractOptions {
    pub out: PathBuf,
    pub conflict: Conflict,
    pub stdout: bool,
    pub copy: bool,
}

pub enum Outcome {
    Text(String),
    Files {
        saved: Vec<PathBuf>,
        skipped: Vec<PathBuf>,
    },
    Stdout,
}

/// Checks the stored body against the manifest's BLAKE3.
pub fn verify_body(store: &Store, manifest: &Manifest) -> Result<()> {
    let mut f = File::open(store.body_path())?;
    let len = f.metadata()?.len();
    if len < manifest.body.length {
        bail!("stored body is incomplete");
    }
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut (&mut f).take(manifest.body.length), &mut hasher)?;
    if hasher.finalize().to_hex().as_str() != manifest.body.blake3 {
        bail!("received data does not match the sender's checksum");
    }
    Ok(())
}

/// The stored body, decrypted when the session is encrypted.
fn body_reader(
    store: &Store,
    manifest: &Manifest,
    me: Option<&DeviceIdentity>,
) -> Result<Box<dyn Read>> {
    let f = File::open(store.body_path())?;
    let raw = BufReader::with_capacity(1 << 20, f.take(manifest.body.length));
    if store.params().flags & FLAG_ENCRYPTED == 0 {
        return Ok(Box::new(raw));
    }
    let me = me.context("this transfer is encrypted, but this device has no identity")?;
    Ok(Box::new(crypto::decrypt_reader(raw, me)?))
}

struct FsSink {
    root: PathBuf,
}

impl UnpackSink for FsSink {
    fn dir(&mut self, path: &SafePath, _: &Entry) -> io::Result<()> {
        fs::create_dir_all(path.join_to(&self.root))
    }

    fn file(&mut self, path: &SafePath, entry: &Entry, content: &mut dyn Read) -> io::Result<()> {
        let target = path.join_to(&self.root);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&target)?);
        io::copy(content, &mut w)?;
        w.flush()?;
        drop(w);
        apply_metadata(&target, entry);
        Ok(())
    }
}

fn apply_metadata(path: &Path, entry: &Entry) {
    #[cfg(unix)]
    if let Some(mode) = entry.mode {
        use std::os::unix::fs::PermissionsExt;
        // Only the executable bit is honoured.
        let perm = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(perm));
    }
    if let Some(mtime) = entry.mtime {
        let _ = filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(mtime, 0));
    }
}

struct StdoutSink;

impl UnpackSink for StdoutSink {
    fn dir(&mut self, _: &SafePath, _: &Entry) -> io::Result<()> {
        Ok(())
    }
    fn file(&mut self, _: &SafePath, _: &Entry, content: &mut dyn Read) -> io::Result<()> {
        let mut out = io::stdout().lock();
        io::copy(content, &mut out)?;
        out.flush()
    }
}

/// `dir/name`, or `dir/name (n)` when that already exists.
pub fn free_path(dir: &Path, name: &str) -> PathBuf {
    let plain = dir.join(name);
    if fs::symlink_metadata(&plain).is_err() {
        plain
    } else {
        free_name(dir, name)
    }
}

fn free_name(dir: &Path, name: &str) -> PathBuf {
    (1..)
        .map(|n| dir.join(numbered_name(name, n)))
        .find(|p| !p.exists() && fs::symlink_metadata(p).is_err())
        .unwrap()
}

/// Moves `src` to `dst` honouring the conflict policy.
fn place(
    src: &Path,
    dst: &Path,
    conflict: Conflict,
    saved: &mut Vec<PathBuf>,
    skipped: &mut Vec<PathBuf>,
) -> Result<()> {
    let exists = fs::symlink_metadata(dst).is_ok();
    if !exists {
        fs::rename(src, dst)?;
        saved.push(dst.to_path_buf());
        return Ok(());
    }
    match conflict {
        Conflict::Skip => skipped.push(dst.to_path_buf()),
        Conflict::Rename => {
            let name = dst.file_name().unwrap().to_string_lossy().into_owned();
            let target = free_name(dst.parent().unwrap(), &name);
            fs::rename(src, &target)?;
            saved.push(target);
        }
        Conflict::Overwrite => {
            let dst_meta = fs::symlink_metadata(dst)?;
            if src.is_dir() && dst_meta.is_dir() {
                for child in fs::read_dir(src)? {
                    let child = child?;
                    place(
                        &child.path(),
                        &dst.join(child.file_name()),
                        conflict,
                        saved,
                        skipped,
                    )?;
                }
                return Ok(());
            }
            if dst_meta.is_dir() {
                fs::remove_dir_all(dst)?;
            } else {
                fs::remove_file(dst)?;
            }
            fs::rename(src, dst)?;
            saved.push(dst.to_path_buf());
        }
    }
    Ok(())
}

pub fn finalize(
    store: &Store,
    manifest: &Manifest,
    me: Option<&DeviceIdentity>,
    opts: &ExtractOptions,
) -> Result<Outcome> {
    verify_body(store, manifest)?;
    let reader = body_reader(store, manifest, me)?;
    match manifest.kind {
        Kind::Text => {
            let text = unpack_text(manifest, reader)?;
            if opts.copy {
                copy_to_clipboard(&text)?;
            }
            Ok(Outcome::Text(text))
        }
        Kind::Files if opts.stdout => {
            let files = manifest
                .entries
                .iter()
                .filter(|e| e.kind == EntryType::File)
                .count();
            if files != 1 {
                bail!("--stdout needs a transfer with exactly one file (this one has {files})");
            }
            unpack(manifest, reader, &mut StdoutSink)?;
            Ok(Outcome::Stdout)
        }
        Kind::Files => {
            fs::create_dir_all(&opts.out)
                .with_context(|| format!("cannot create {}", opts.out.display()))?;
            let partial = opts.out.join(format!(
                ".qrsend-{}.partial",
                session_hex(store.state.session_id)
            ));
            if partial.exists() {
                fs::remove_dir_all(&partial)?;
            }
            fs::create_dir_all(&partial)?;
            if let Err(e) = unpack(
                manifest,
                reader,
                &mut FsSink {
                    root: partial.clone(),
                },
            ) {
                let _ = fs::remove_dir_all(&partial);
                return Err(e.into());
            }
            let (mut saved, mut skipped) = (Vec::new(), Vec::new());
            let mut tops: Vec<_> = fs::read_dir(&partial)?.collect::<Result<_, _>>()?;
            tops.sort_by_key(|e| e.file_name());
            for top in tops {
                place(
                    &top.path(),
                    &opts.out.join(top.file_name()),
                    opts.conflict,
                    &mut saved,
                    &mut skipped,
                )?;
            }
            fs::remove_dir_all(&partial)?;
            Ok(Outcome::Files { saved, skipped })
        }
    }
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("pbcopy", &[])]
    } else if cfg!(windows) {
        &[("clip", &[])]
    } else {
        &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ]
    };
    for (cmd, args) in candidates {
        let Ok(mut child) = Command::new(cmd).args(*args).stdin(Stdio::piped()).spawn() else {
            continue;
        };
        child.stdin.take().unwrap().write_all(text.as_bytes())?;
        if child.wait()?.success() {
            return Ok(());
        }
    }
    bail!("no clipboard tool found (install wl-copy, xclip or xsel)")
}
