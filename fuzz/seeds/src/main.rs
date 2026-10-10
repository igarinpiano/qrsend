//! Seed inputs for the fuzz targets, one directory per target.

use std::fs;
use std::path::{Path, PathBuf};

use qrsend_core::crypto::DeviceIdentity;
use qrsend_core::direct;
use qrsend_core::feedback::{Camera, Feedback, SenderNotice};
use qrsend_core::link::{self, TcpOffer};
use qrsend_core::manifest::{Body, Kind, Manifest, MetaEnvelope, session_hex};
use qrsend_core::payload::{BodyHasher, PackOptions, Packer};
use qrsend_core::resume::ResumeCode;
use qrsend_core::schedule::ScheduleConfig;
use qrsend_core::sender::{MemorySource, Sender, SessionLayout};

fn write(root: &Path, target: &str, name: &str, data: &[u8]) {
    let dir = root.join(target);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), data).unwrap();
}

/// A small transfer: its manifest, meta segment and body.
fn transfer(
    id: u32,
    compress: bool,
    signer: Option<&DeviceIdentity>,
) -> (Manifest, Vec<u8>, Vec<u8>) {
    let opts = PackOptions {
        zstd_level: compress.then_some(3),
        zstd_workers: 0,
    };
    let mut packer = Packer::new(BodyHasher::new(Vec::new(), 12), opts).unwrap();
    packer.add_dir("docs", Some(1_700_000_000));
    packer
        .add_file(
            "docs/a.txt",
            &mut &b"hello, world\n"[..],
            None,
            Some(0o644),
            Some(1),
        )
        .unwrap();
    packer
        .add_file("docs/A.TXT", &mut &[7u8; 5000][..], None, Some(0o755), None)
        .unwrap();
    packer
        .add_file("empty", &mut &b""[..], None, None, None)
        .unwrap();
    let (hasher, packed) = packer.finish().unwrap();
    let (body, digest) = hasher.finish().unwrap();
    let manifest = Manifest {
        qrsend: 1,
        session: session_hex(id),
        kind: Kind::Files,
        created: 1_790_000_000,
        sender_name: Some("seed".into()),
        body: Body {
            length: digest.length,
            blake3: digest.blake3,
            segment_blake3: digest.segment_blake3,
            encoding: packed.encoding,
            plain_length: packed.plain_length,
        },
        entries: packed.entries,
    };
    let mut env = MetaEnvelope::from_manifest(&manifest).unwrap();
    if let Some(me) = signer {
        env.signature = Some(me.sign_meta(id, &env.manifest_z));
    }
    (manifest, env.encode(), body)
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "corpus".into()));
    let me = DeviceIdentity::generate("seed").unwrap();

    // meta: the session id in the first byte, then the meta segment.
    for (n, (compress, signer)) in [(true, None), (false, Some(&me))].into_iter().enumerate() {
        let (_, meta, _) = transfer(42, compress, signer);
        write(
            &root,
            "meta",
            &format!("meta-{n}"),
            &[&[42u8][..], &meta].concat(),
        );
    }

    // payload: manifest JSON, a zero byte, the body (files and a text).
    for (n, compress) in [true, false].into_iter().enumerate() {
        let (m, _, body) = transfer(7, compress, None);
        let json = serde_json::to_vec(&m).unwrap();
        write(
            &root,
            "payload",
            &format!("files-{n}"),
            &[json, vec![0], body].concat(),
        );
    }
    let (text_body, packed) = Packer::text(Vec::new(), "a text", PackOptions::default()).unwrap();
    let text = Manifest {
        qrsend: 1,
        session: session_hex(8),
        kind: Kind::Text,
        created: 0,
        sender_name: None,
        body: Body {
            length: text_body.len() as u64,
            blake3: String::new(),
            segment_blake3: vec![],
            encoding: packed.encoding,
            plain_length: packed.plain_length,
        },
        entries: vec![],
    };
    let json = serde_json::to_vec(&text).unwrap();
    write(
        &root,
        "payload",
        "text",
        &[json, vec![0], text_body].concat(),
    );

    // frame and receiver: the frames of a transfer, in two code sizes.
    let (_, meta, body) = transfer(0x51, true, Some(&me));
    let mut stream = Vec::new();
    for symbol_size in [64, 181] {
        let layout = SessionLayout {
            session_id: 0x51,
            flags: 0,
            seg_shift: 12,
            meta_len: meta.len() as u32,
            body_len: body.len() as u64,
            symbol_size,
        };
        let source = MemorySource {
            meta: meta.clone(),
            body: body.clone(),
            seg_shift: 12,
        };
        let mut sender = Sender::new(layout, source, ScheduleConfig::default(), None);
        for i in 0..sender.total_symbols() * 2 {
            let frame = sender.next_frame().unwrap().encode();
            if i < 8 {
                write(&root, "frame", &format!("frame-{symbol_size}-{i}"), &frame);
            }
            stream.extend_from_slice(&(frame.len() as u16).to_le_bytes());
            stream.extend_from_slice(&frame);
        }
    }
    write(&root, "receiver", "transfer", &stream);

    // codes: feedback, notices, resume codes, link codes and records.
    let feedback = Feedback {
        session_id: 0x51,
        seq: 3,
        complete: false,
        truncated: true,
        frames: 1234,
        remaining_symbols: 99,
        symbol_size: 181,
        missing: vec![(0, 1), (4, 2), (9, 1)],
        camera: Camera {
            reads_tenths: 143,
            dot_tenths: 52,
            colors: 0b101_110,
        },
    };
    write(&root, "codes", "feedback-bytes", &feedback.to_bytes());
    write(&root, "codes", "feedback", feedback.encode().as_bytes());
    let notice = SenderNotice {
        session_id: 0x51,
        wants_feedback: true,
        hears_sound: true,
        colors: 0b111,
    };
    write(&root, "codes", "notice", notice.encode().as_bytes());
    let resume = ResumeCode::new(0x51, vec![0, 3, 4, 5, 9, 70_000]);
    write(&root, "codes", "resume", resume.encode().as_bytes());
    let offer = TcpOffer {
        port: 40123,
        key: [9; 16],
        addresses: vec!["192.168.1.20".into(), "fd00::1".into()],
    };
    write(&root, "codes", "tcp-offer", &offer.to_bytes());
    let parts = link::split(0x51, link::KIND_TCP_OFFER, 1, &offer.to_bytes(), 40).unwrap();
    write(&root, "codes", "link-parts", parts.join("\n").as_bytes());
    let mut records = Vec::new();
    direct::pack(
        &mut records,
        &stream[2..2 + u16::from_le_bytes([stream[0], stream[1]]) as usize],
    );
    direct::pack(&mut records, notice.encode().as_bytes());
    write(&root, "codes", "records", &records);

    // sound: a feedback message as a microphone would hear it.
    for (n, rate) in [(0u8, 8_000.0), (3, 48_000.0)] {
        let samples = qrsend_core::sound::encode(&feedback.to_bytes(), rate);
        let mut data = vec![n];
        for s in samples {
            data.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        write(&root, "sound", &format!("feedback-{rate}"), &data);
    }
}
