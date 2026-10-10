//! The meta segment: envelope, signature, compressed manifest, the paths it
//! names. (Unencrypted: decryption is age's, fuzzed by its own project.)
#![no_main]

use libfuzzer_sys::fuzz_target;
use qrsend_core::crypto;
use qrsend_core::payload;
use qrsend_core::sanitize::SafePath;

fuzz_target!(|data: &[u8]| {
    let Some((&id, meta)) = data.split_first() else {
        return;
    };
    let Ok(opened) = crypto::open_meta(meta, id as u32, false, None) else {
        return;
    };
    let m = &opened.manifest;
    let _ = m.files_total();
    if let Ok(paths) = payload::plan_paths(m) {
        for p in paths {
            for c in p.components() {
                assert!(!c.is_empty() && c != "." && c != "..");
                assert!(!c.contains(['/', '\\', '\0', ':']));
            }
            // A safe path stays the same path when read again.
            assert_eq!(SafePath::parse(&p.to_string()).unwrap(), p);
        }
    }
});
