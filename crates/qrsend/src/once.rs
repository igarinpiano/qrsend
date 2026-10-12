//! One-time keys this device has shown to senders and is not done with yet
//! (forward secrecy, docs/PROTOCOL.md §9.2). The secret half of each is
//! kept until the transfer it was shown for has arrived, and destroyed
//! then: from that moment nothing on this machine opens that transfer as
//! it was sent, whatever becomes of the device's lasting keys.
//!
//! A key nobody used, or whose transfer never finished, is destroyed after
//! a day: a transfer that stopped halfway has to be finished within that.

use std::path::PathBuf;

use anyhow::Result;
use qrsend_core::pq::OnceSecret;

use crate::{paths, util};

/// How long a one-time key is kept at most.
const KEPT_SECS: i64 = 24 * 3600;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    (text.len() % 2 == 0).then_some(())?;
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn path() -> PathBuf {
    paths::data_dir().join("one-time-keys")
}

/// The keys kept (when each was made), without those that are too old:
/// those are gone from the file after this.
pub fn pending() -> Vec<(i64, OnceSecret)> {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Vec::new();
    };
    let now = util::now();
    let mut all = 0;
    let kept: Vec<(i64, OnceSecret)> = text
        .lines()
        .filter_map(|line| {
            all += 1;
            let (made, secret) = line.split_once(' ')?;
            let secret = OnceSecret::from_bytes(&unhex(secret)?)?;
            Some((made.parse().ok()?, secret))
        })
        .filter(|(made, _)| now - made < KEPT_SECS)
        .collect();
    if kept.len() != all {
        let _ = write(&kept);
    }
    kept
}

fn write(keys: &[(i64, OnceSecret)]) -> Result<()> {
    if keys.is_empty() {
        let _ = std::fs::remove_file(path());
        return Ok(());
    }
    let text: String = keys
        .iter()
        .map(|(made, secret)| format!("{made} {}\n", hex(&secret.to_bytes())))
        .collect();
    paths::create_private(&paths::data_dir())?;
    paths::write_private(&path(), text.as_bytes())?;
    Ok(())
}

/// Keeps the secret half of a key that is about to be shown.
pub fn keep(secret: &OnceSecret) -> Result<()> {
    let mut keys = pending();
    keys.push((util::now(), secret.clone()));
    write(&keys)
}

/// Destroys the keys with these names (see `OncePublic::id`): the transfer
/// they were shown for has arrived.
pub fn destroy(ids: &[[u8; 8]]) {
    if ids.is_empty() {
        return;
    }
    let keys = pending();
    let left: Vec<_> = keys
        .iter()
        .filter(|(_, secret)| !ids.contains(&secret.public().id()))
        .cloned()
        .collect();
    if left.len() != keys.len() {
        let _ = write(&left);
    }
}
