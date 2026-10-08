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
        symbol_size: 0,
        missing: vec![],
        camera: Default::default(),
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

/// Frames of two symbol sizes may arrive for one segment (two channels at
/// once). Strays of another size must not throw away progress; a channel that
/// brings more takes over.
#[test]
fn a_segment_is_collected_in_the_size_that_brings_most() {
    let body = noise(40_000, 11);
    let meta = noise(100, 12);
    let layout = |symbol_size| SessionLayout {
        session_id: 5,
        flags: 0,
        seg_shift: 16,
        meta_len: meta.len() as u32,
        body_len: body.len() as u64,
        symbol_size,
    };
    let source = || MemorySource {
        meta: meta.clone(),
        body: body.clone(),
        seg_shift: 16,
    };
    let quiet = ScheduleConfig {
        meta_interval: 0,
        ..ScheduleConfig::default()
    };
    let mut narrow = Sender::new(layout(100), source(), quiet.clone(), Some(&[1]));
    let mut wide = Sender::new(layout(2000), source(), quiet, Some(&[1]));
    let body_frame = |s: &mut Sender<MemorySource>| loop {
        let f = s.next_frame().unwrap();
        if f.header.seg_index == 1 {
            return f;
        }
    };
    let mut rx = Receiver::new();
    for _ in 0..200 {
        rx.push(body_frame(&mut narrow));
    }
    assert_eq!(rx.partial(), vec![(1, 200, 400)]);
    // 20 kB are collected; 10 kB of the other size going by change nothing.
    for _ in 0..5 {
        rx.push(body_frame(&mut wide));
    }
    rx.push(body_frame(&mut narrow));
    assert_eq!(rx.partial(), vec![(1, 201, 400)]);
    // The other channel keeps delivering, mixed with the first: once it has
    // brought more, the segment is collected in its size and soon complete.
    let mut done = false;
    for _ in 0..40 {
        rx.push(body_frame(&mut narrow));
        for ev in rx.push(body_frame(&mut wide)) {
            if let Event::Completed { index: 1, data } = ev {
                assert_eq!(data, body);
                done = true;
            }
        }
    }
    assert!(done);
    // Progress is counted in the size that carried the data.
    assert_eq!(rx.progress().unwrap().symbol_size, 2000);
}

/// Continuing with a resume code: only the missing segments are sent, and
/// each from its end, so that a receiver which has the beginning of one
/// needs exactly the rest of it.
#[test]
fn a_resumed_sender_brings_what_is_missing_at_once() {
    let body = noise(300_000, 21);
    let meta = noise(200, 22);
    let seg_shift = 16;
    let layout = SessionLayout {
        session_id: 77,
        flags: 0,
        seg_shift,
        meta_len: meta.len() as u32,
        body_len: body.len() as u64,
        symbol_size: 500,
    };
    let source = || MemorySource {
        meta: meta.clone(),
        body: body.clone(),
        seg_shift,
    };
    let quiet = ScheduleConfig {
        meta_interval: 0,
        window: 2,
        ..ScheduleConfig::default()
    };
    // The first attempt stops in its second window (two segments at a
    // time): segment 1 is there, segments 2 and 3 to 60 %.
    let mut rx = Receiver::new();
    let mut first = Sender::new(layout, source(), quiet.clone(), None);
    let k = layout.k(2) as usize;
    let mut of_each = [0usize; 6];
    while of_each[2] < k * 6 / 10 || of_each[3] < k * 6 / 10 {
        let f = first.next_frame().unwrap();
        of_each[f.header.seg_index as usize] += 1;
        rx.push(f);
    }
    assert!(rx.is_done(0) && rx.is_done(1) && !rx.is_done(2) && !rx.is_done(3));
    let code = crate::resume::ResumeCode::new(77, rx.missing());
    assert_eq!(code.segments, vec![2, 3, 4, 5]);

    // Sent again in the usual order, the first 60 % of segments 2 and 3 would
    // repeat what is there. With the code:
    let mut again = Sender::new(layout, source(), quiet, Some(&code.segments));
    again.set_from_the_end(true);
    // How many symbols of each segment it took until that segment was there.
    let mut took = [0usize; 6];
    let mut frames = 0;
    while !(rx.is_done(2) && rx.is_done(3)) {
        let f = again.next_frame().unwrap();
        let index = f.header.seg_index as usize;
        assert_ne!(index, 1, "a segment that is not missing");
        if !rx.is_done(f.header.seg_index) {
            took[index] += 1;
        }
        rx.push(f);
        frames += 1;
        assert!(frames < 8 * k, "never completes");
    }
    // Each lacked 40 %: that is all it took, the first symbol on.
    let lacking = k - k * 6 / 10;
    assert!(
        took[2] <= lacking + 1,
        "{} symbols for {lacking} lacking",
        took[2]
    );
    assert!(
        took[3] <= lacking + 1,
        "{} symbols for {lacking} lacking",
        took[3]
    );
}
