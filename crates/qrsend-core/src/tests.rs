//! End-to-end round trips through every layer, in memory.

use std::io::Read;

use crate::frame::Frame;
use crate::manifest::{Body, Kind, Manifest, MetaEnvelope, session_hex};
use crate::payload::{BodyHasher, PackOptions, Packer, UnpackSink, segment_hash, unpack};
use crate::receiver::{Event, Receiver};
use crate::sanitize::SafePath;
use crate::schedule::ScheduleConfig;
use crate::sender::{MemorySource, Sender, SessionLayout};

struct Collect(Vec<(String, Vec<u8>)>);

impl UnpackSink for Collect {
    fn dir(&mut self, _: &SafePath, _: &crate::manifest::Entry) -> std::io::Result<()> {
        Ok(())
    }
    fn file(
        &mut self,
        p: &SafePath,
        _: &crate::manifest::Entry,
        r: &mut dyn Read,
    ) -> std::io::Result<()> {
        let mut v = Vec::new();
        r.read_to_end(&mut v)?;
        self.0.push((p.to_string(), v));
        Ok(())
    }
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

#[test]
fn full_pipeline_with_loss() {
    let files = vec![
        ("a.bin", noise(70_000, 1)),
        ("dir/b.txt", b"hello world".repeat(3000)),
        ("c", vec![]),
    ];
    let seg_shift = 14; // 16 KiB segments -> several body segments
    let session_id = 0x0BAD_F00D;

    let mut packer = Packer::new(
        BodyHasher::new(Vec::new(), seg_shift),
        PackOptions::default(),
    )
    .unwrap();
    for (path, data) in &files {
        packer
            .add_file(path, &mut &data[..], Some(data.len() as u64), None, None)
            .unwrap();
    }
    let (hasher, packed) = packer.finish().unwrap();
    let (body, digest) = hasher.finish().unwrap();
    let manifest = Manifest {
        qrsend: 1,
        session: session_hex(session_id),
        kind: Kind::Files,
        created: 0,
        sender_name: None,
        body: Body {
            length: digest.length,
            blake3: digest.blake3.clone(),
            segment_blake3: digest.segment_blake3.clone(),
            encoding: packed.encoding,
            plain_length: packed.plain_length,
        },
        entries: packed.entries,
    };
    let meta = MetaEnvelope::from_manifest(&manifest).unwrap().encode();

    let layout = SessionLayout {
        session_id,
        flags: 0,
        seg_shift,
        meta_len: meta.len() as u32,
        body_len: body.len() as u64,
        symbol_size: 300,
    };
    assert!(layout.seg_count() >= 2, "test should span several segments");
    let source = MemorySource {
        meta: meta.clone(),
        body: body.clone(),
        seg_shift,
    };
    let mut sender = Sender::new(layout, source, ScheduleConfig::default(), None);

    let mut rx = Receiver::new();
    assert_eq!(rx.progress(), None);
    let mut last_remaining = u64::MAX;
    let mut got_body = vec![0u8; body.len()];
    let mut got_meta = None;
    let mut sent = 0u64;
    while !rx.is_complete() {
        let frame = sender.next_frame().unwrap();
        sent += 1;
        assert!(sent < 10_000, "transfer did not converge");
        if sent.is_multiple_of(4) || sent.is_multiple_of(7) {
            continue; // ~36% frame loss
        }
        if let Some(p) = rx.progress() {
            // The remaining work only ever shrinks.
            assert!(p.remaining_bytes <= last_remaining && p.remaining_bytes <= p.total_bytes);
            last_remaining = p.remaining_bytes;
        }
        let text = frame.to_qr_text();
        for ev in rx.push(Frame::from_qr_text(&text).unwrap()) {
            if let Event::Completed { index, data } = ev {
                if index == 0 {
                    got_meta = Some(data);
                } else {
                    assert_eq!(
                        segment_hash(&data),
                        digest.segment_blake3[index as usize - 1]
                    );
                    let off = rx.params().unwrap().offset(index) as usize;
                    got_body[off..off + data.len()].copy_from_slice(&data);
                }
            }
        }
    }
    let done = rx.progress().unwrap();
    assert_eq!((done.remaining_bytes, done.remaining_symbols), (0, 0));
    assert_eq!(done.total_bytes, (body.len() + meta.len()) as u64);
    assert_eq!(got_body, body);
    let decoded = MetaEnvelope::decode(&got_meta.unwrap())
        .unwrap()
        .manifest(session_id)
        .unwrap();
    assert_eq!(decoded, manifest);

    let mut out = Collect(Vec::new());
    unpack(&decoded, &got_body[..], &mut out).unwrap();
    let expected: Vec<(String, Vec<u8>)> =
        files.into_iter().map(|(p, d)| (p.to_string(), d)).collect();
    assert_eq!(out.0, expected);
}

#[test]
fn receiver_ignores_foreign_sessions() {
    let make = |sid| {
        let layout = SessionLayout {
            session_id: sid,
            flags: 0,
            seg_shift: 12,
            meta_len: 10,
            body_len: 0,
            symbol_size: 16,
        };
        Sender::new(
            layout,
            MemorySource {
                meta: vec![1; 10],
                body: vec![],
                seg_shift: 12,
            },
            ScheduleConfig::default(),
            None,
        )
    };
    let mut rx = Receiver::new();
    let mut a = make(1);
    let mut b = make(2);
    assert!(matches!(
        rx.push(a.next_frame().unwrap())[..],
        [Event::Locked(_), ..]
    ));
    assert_eq!(
        rx.push(b.next_frame().unwrap()),
        vec![Event::ForeignSession(2)]
    );
    assert!(rx.push(b.next_frame().unwrap()).is_empty());
}
