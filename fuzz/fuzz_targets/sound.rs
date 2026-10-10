//! The sound decoder, listening to whatever a microphone picks up.
#![no_main]

use libfuzzer_sys::fuzz_target;
use qrsend_core::sound::Decoder;

fuzz_target!(|data: &[u8]| {
    let Some((&rate, samples)) = data.split_first() else {
        return;
    };
    let rate = [8_000.0, 16_000.0, 44_100.0, 48_000.0][rate as usize % 4];
    let samples: Vec<f32> = samples
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
        .collect();
    let mut decoder = Decoder::new(rate);
    for chunk in samples.chunks(128) {
        for message in decoder.push(chunk) {
            assert!(message.len() <= qrsend_core::sound::MAX_PAYLOAD);
        }
    }
});
