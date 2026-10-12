//! The small codes besides frames: feedback, notices, resume codes, link
//! codes (and the offers in them), records off a network connection, and
//! device IDs (as text, as bytes, and in the parts they are shown as).
#![no_main]

use libfuzzer_sys::fuzz_target;
use qrsend_core::crypto::{DevicePublic, IdReader};
use qrsend_core::direct::{self, Record};
use qrsend_core::feedback::{Feedback, SenderNotice};
use qrsend_core::link::{self, LinkPart, TcpOffer};
use qrsend_core::resume::ResumeCode;

fuzz_target!(|data: &[u8]| {
    if let Ok(f) = Feedback::from_bytes(data) {
        // Whatever reads, asks no impossible question of the sender.
        for i in [0, 1, 2, 1 << 24] {
            let _ = f.needs(i);
        }
        let again = Feedback::from_bytes(&f.to_bytes()).unwrap();
        assert_eq!(again.session_id, f.session_id);
    }
    let _ = TcpOffer::from_bytes(data);
    // An ID that is taken says the same when written out again.
    if let Ok(device) = DevicePublic::from_bytes(data) {
        let again = DevicePublic::from_bytes(&device.to_bytes().unwrap()).unwrap();
        assert_eq!(again.fingerprint(), device.fingerprint());
    }
    for record in direct::unpack(data) {
        let _ = Record::parse(record);
    }
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = Feedback::decode(text);
        let _ = SenderNotice::decode(text);
        if let Ok(code) = ResumeCode::decode(text) {
            assert!(code.segments.windows(2).all(|w| w[0] < w[1]));
        }
        if let Ok(part) = LinkPart::decode(text) {
            assert!(part.part < part.parts);
        }
        // Parts of messages, one per line, in any order and repeated.
        let mut assembler = link::Assembler::default();
        let mut ids = IdReader::default();
        for line in text.lines() {
            let _ = assembler.add(line);
            let _ = ids.add(line);
        }
        if let Ok(device) = DevicePublic::parse(text) {
            let again = DevicePublic::parse(&device.to_id_string()).unwrap();
            assert_eq!(again.fingerprint(), device.fingerprint());
        }
    }
});
