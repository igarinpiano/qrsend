//! Turns command-line inputs into an ordered list of entries to pack.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub enum Source {
    File(PathBuf),
    Stdin,
}

pub struct Item {
    /// `/`-separated path inside the transfer.
    pub rel: String,
    pub source: Option<Source>,
    pub size: Option<u64>,
    pub mode: Option<u32>,
    pub mtime: Option<i64>,
}

fn name_of(path: &Path) -> Result<String> {
    let abs = if path.file_name().is_none() {
        path.canonicalize()?
    } else {
        path.to_path_buf()
    };
    let name = abs
        .file_name()
        .with_context(|| format!("{} has no file name", path.display()))?;
    Ok(name.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn mode_of(_: &fs::Metadata) -> Option<u32> {
    None
}

fn mtime_of(meta: &fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

fn walk(path: &Path, rel: String, items: &mut Vec<Item>, warnings: &mut Vec<String>) -> Result<()> {
    let meta =
        fs::symlink_metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        warnings.push(format!("skipping symlink {}", path.display()));
    } else if ft.is_dir() {
        items.push(Item {
            rel: rel.clone(),
            source: None,
            size: None,
            mode: None,
            mtime: mtime_of(&meta),
        });
        let mut children: Vec<_> = fs::read_dir(path)
            .with_context(|| format!("cannot list {}", path.display()))?
            .collect::<Result<_, _>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name();
            let name_str = name.to_string_lossy();
            if name_str.contains('\u{FFFD}') {
                warnings.push(format!(
                    "non-UTF-8 name {} was converted",
                    child.path().display()
                ));
            }
            walk(&child.path(), format!("{rel}/{name_str}"), items, warnings)?;
        }
    } else if ft.is_file() {
        items.push(Item {
            rel,
            source: Some(Source::File(path.to_path_buf())),
            size: Some(meta.len()),
            mode: mode_of(&meta),
            mtime: mtime_of(&meta),
        });
    } else {
        warnings.push(format!("skipping special file {}", path.display()));
    }
    Ok(())
}

/// Collects files and directories (recursively, sorted). `-` means stdin.
pub fn collect(paths: &[PathBuf], stdin_name: &str) -> Result<(Vec<Item>, Vec<String>)> {
    let mut items = Vec::new();
    let mut warnings = Vec::new();
    for p in paths {
        if p.as_os_str() == "-" {
            items.push(Item {
                rel: stdin_name.to_string(),
                source: Some(Source::Stdin),
                size: None,
                mode: None,
                mtime: None,
            });
        } else {
            walk(p, name_of(p)?, &mut items, &mut warnings)?;
        }
    }
    let mut seen = HashSet::new();
    for item in &items {
        if !seen.insert(item.rel.as_str()) {
            bail!(
                "two inputs share the name {:?}; rename one or put them in a folder",
                item.rel
            );
        }
    }
    if items.is_empty() {
        bail!("nothing to send");
    }
    Ok((items, warnings))
}
