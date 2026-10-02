//! QR detection for the CLI: ZXing (rxing) first — robust on camera footage,
//! screenshots and scaled video — then rqrr as a second opinion.

use std::collections::HashSet;

use qrsend_core::qr::{self, Luma};
use rxing::{BarcodeFormat, DecodeHints};

use crate::input::LumaFrame;

pub fn detect(f: &LumaFrame) -> Vec<String> {
    let mut hints = DecodeHints {
        PossibleFormats: Some(HashSet::from([BarcodeFormat::QR_CODE])),
        TryHarder: Some(true),
        ..DecodeHints::default()
    };
    let found: Vec<String> = rxing::helpers::detect_multiple_in_luma_with_hints(
        f.pixels.clone(),
        f.width as u32,
        f.height as u32,
        &mut hints,
    )
    .map(|results| results.iter().map(|r| r.getText().to_string()).collect())
    .unwrap_or_default();
    if !found.is_empty() {
        return found;
    }
    qr::detect(Luma {
        width: f.width,
        height: f.height,
        pixels: &f.pixels,
    })
}
