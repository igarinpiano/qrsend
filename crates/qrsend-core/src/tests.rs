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
            assert!(p.remaining_symbols <= p.total_symbols);
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

/// Frames a lossy transfer takes, with or without the receiver answering.
fn frames_until_complete(with_feedback: bool, lose: impl Fn(u64) -> bool) -> u64 {
    let seg_shift = 13;
    let body = noise(200_000, 9);
    let meta = noise(300, 3);
    let layout = SessionLayout {
        session_id: 77,
        flags: 0,
        seg_shift,
        meta_len: meta.len() as u32,
        body_len: body.len() as u64,
        symbol_size: 200,
    };
    let source = MemorySource {
        meta,
        body: body.clone(),
        seg_shift,
    };
    let mut sender = Sender::new(layout, source, ScheduleConfig::default(), None);
    let mut rx = Receiver::new();
    let mut got = vec![0u8; body.len()];
    let mut sent = 0u64;
    loop {
        let frame = sender.next_frame().unwrap();
        sent += 1;
        assert!(sent < 100_000, "transfer did not converge");
        if !lose(sent) {
            for ev in rx.push(frame) {
                if let Event::Completed { index, data } = ev
                    && index > 0
                {
                    let off = rx.params().unwrap().offset(index) as usize;
                    got[off..off + data.len()].copy_from_slice(&data);
                }
            }
        }
        // The answer reaches the sender a little later, and through text.
        if with_feedback && sent.is_multiple_of(25) {
            let fb = rx.feedback(rx.is_complete());
            if let Some(fb) = fb {
                let fb = crate::feedback::Feedback::decode(&fb.encode()).unwrap();
                let complete = fb.complete;
                assert!(sender.apply_feedback(fb));
                if complete {
                    break;
                }
            }
        } else if !with_feedback && rx.is_complete() {
            break;
        }
    }
    assert_eq!(got, body);
    sent
}

#[test]
fn feedback_saves_frames() {
    // No loss: the redundancy of a pass is not sent once a segment is acknowledged.
    let (blind, guided) = (
        frames_until_complete(false, |_| false),
        frames_until_complete(true, |_| false),
    );
    assert!(guided <= blind + 25, "{guided} vs {blind}");

    // Heavy loss (40%): without feedback every segment needs a second pass;
    // with it each window is fed until it is complete.
    let lose = |n: u64| n % 5 < 2;
    let (blind, guided) = (
        frames_until_complete(false, lose),
        frames_until_complete(true, lose),
    );
    assert!(guided * 10 < blind * 9, "{guided} vs {blind}");
    eprintln!("40% loss: {blind} frames blind, {guided} with feedback");
}

#[test]
fn feedback_for_another_session_is_ignored() {
    let layout = SessionLayout {
        session_id: 1,
        flags: 0,
        seg_shift: 12,
        meta_len: 10,
        body_len: 5000,
        symbol_size: 100,
    };
    let source = MemorySource {
        meta: vec![0; 10],
        body: vec![0; 5000],
        seg_shift: 12,
    };
    let mut sender = Sender::new(layout, source, ScheduleConfig::default(), None);
    let per_pass = sender.frames_per_pass();
    let other = crate::feedback::Feedback {
        session_id: 2,
        seq: 1,
        complete: true,
        truncated: false,
        frames: 0,
        remaining_symbols: 0,
        missing: vec![],
    };
    assert!(!sender.apply_feedback(other.clone()));
    assert!(sender.feedback().is_none());
    assert_eq!(sender.frames_per_pass(), per_pass);
    assert!(sender.apply_feedback(crate::feedback::Feedback {
        session_id: 1,
        missing: vec![(2, 1)],
        ..other
    }));
    assert!(sender.frames_per_pass() < per_pass);
    sender.forget_receiver();
    assert_eq!(sender.frames_per_pass(), per_pass);
}

/// The back channel may drop out at any moment; the transfer must not depend
/// on it. Here the receiver even loses everything it had (think of a page
/// reload without storage) right when its feedback stops being read.
#[test]
fn transfer_survives_losing_the_back_channel() {
    let seg_shift = 12;
    let body = noise(60_000, 5);
    let meta = noise(200, 6);
    let layout = SessionLayout {
        session_id: 99,
        flags: 0,
        seg_shift,
        meta_len: meta.len() as u32,
        body_len: body.len() as u64,
        symbol_size: 100,
    };
    let source = MemorySource {
        meta,
        body: body.clone(),
        seg_shift,
    };
    let mut sender = Sender::new(layout, source, ScheduleConfig::default(), None);
    let mut rx = Receiver::new();
    let mut got = vec![0u8; body.len()];
    let mut sent = 0u64;
    let (cut, quiet_after, lost_after) = (300, 60, 300);
    while !rx.is_complete() {
        let frame = sender.next_frame().unwrap();
        sent += 1;
        assert!(sent < 20_000, "transfer did not converge");
        if sent == cut {
            assert!(
                rx.completed_count() > 2,
                "the first part should have arrived"
            );
            rx = Receiver::new();
        }
        for ev in rx.push(frame) {
            if let Event::Completed { index, data } = ev
                && index > 0
            {
                let off = rx.params().unwrap().offset(index) as usize;
                got[off..off + data.len()].copy_from_slice(&data);
            }
        }
        if sent < cut && sent.is_multiple_of(20) {
            let fb = rx.feedback(false).unwrap();
            assert!(sender.apply_feedback(fb));
        } else if sent == cut + quiet_after {
            sender.receiver_quiet();
        } else if sent == cut + lost_after {
            // Until now the sender still trusted the last report.
            assert!(sender.feedback().is_some());
            sender.forget_receiver();
        }
    }
    assert_eq!(got, body);
}
