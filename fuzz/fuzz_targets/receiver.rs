//! The receiving state machine, fed frames that need not belong together:
//! other sessions, other symbol sizes, contradicting lengths, repeats.
#![no_main]

use libfuzzer_sys::fuzz_target;
use qrsend_core::frame::Frame;
use qrsend_core::receiver::{Event, Receiver};

fuzz_target!(|data: &[u8]| {
    let mut rx = Receiver::new().with_max_decoders(4);
    // Frames one after another, each after its length (two bytes).
    let mut rest = data;
    while let Some((len, tail)) = rest.split_first_chunk::<2>() {
        let len = (u16::from_le_bytes(*len) as usize).min(tail.len());
        let (bytes, tail) = tail.split_at(len);
        rest = tail;
        let Ok(frame) = Frame::decode(bytes) else {
            continue;
        };
        for event in rx.push(frame) {
            if let Event::Completed { index, .. } = event
                && index % 3 == 0
            {
                // Some segments fail verification and are collected again.
                rx.reset(index);
            }
        }
        if let Some(p) = rx.progress() {
            assert!(p.remaining_bytes <= p.total_bytes);
            assert!(p.remaining_symbols <= p.total_symbols);
        }
        let _ = rx.feedback(false).map(|f| f.encode());
        let _ = rx.partial();
    }
});
