//! Unpacking a body by what a manifest says: sizes, hashes, paths, zstd.
#![no_main]

use std::io::{self, Read};

use libfuzzer_sys::fuzz_target;
use qrsend_core::manifest::{Entry, Manifest, MetaEnvelope};
use qrsend_core::payload::{self, UnpackSink};
use qrsend_core::sanitize::SafePath;

/// Takes everything in, keeps nothing, and holds the paths to their promise.
struct Counting(u64);

impl UnpackSink for Counting {
    fn dir(&mut self, path: &SafePath, _: &Entry) -> io::Result<()> {
        assert!(
            !path
                .to_string()
                .split('/')
                .any(|c| c == ".." || c.is_empty())
        );
        Ok(())
    }
    fn file(&mut self, _: &SafePath, entry: &Entry, content: &mut dyn Read) -> io::Result<()> {
        let n = io::copy(content, &mut io::sink())?;
        assert!(n <= entry.file_size());
        self.0 += n;
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    // A manifest in JSON, a zero byte, then the body.
    let Some(at) = data.iter().position(|&b| b == 0) else {
        return;
    };
    let Some(m) = checked(&data[..at]) else {
        return;
    };
    let body = &data[at + 1..];
    let mut sink = Counting(0);
    if payload::unpack(&m, body, &mut sink).is_ok() {
        assert_eq!(Some(sink.0), m.files_total());
    }
    let _ = payload::unpack_text(&m, body);
});

/// The manifest as a receiver gets it: only what passes the checks of the
/// meta envelope is unpacked.
fn checked(json: &[u8]) -> Option<Manifest> {
    let m: Manifest = serde_json::from_slice(json).ok()?;
    let id = m.session_id()?;
    MetaEnvelope::from_manifest(&m).ok()?.manifest(id).ok()
}
