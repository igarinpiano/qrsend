//! A frame, as text off a QR code and as bytes off a network connection.
#![no_main]

use libfuzzer_sys::fuzz_target;
use qrsend_core::frame::Frame;

fuzz_target!(|data: &[u8]| {
    if let Ok(frame) = Frame::decode(data) {
        // What decodes, encodes back to the same bytes.
        assert_eq!(frame.encode(), data);
        assert_eq!(
            Frame::from_qr_text(&frame.to_qr_text()).as_ref(),
            Ok(&frame)
        );
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = Frame::from_qr_text(text);
        let _ = qrsend_core::base45::decode(text);
    }
});
