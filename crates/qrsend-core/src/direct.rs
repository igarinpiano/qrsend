//! Sending over a channel that loses nothing and keeps the order (a network
//! connection). See docs/PROTOCOL.md §13.1.
//!
//! The fountain code is of no use there: every segment is sent as its source
//! symbols, once, in order, which costs neither side any coding work. The
//! frames are the usual ones, so a receiver needs to know nothing about it.
//!
//! Repair symbols still have their place. A receiver collects a segment in
//! one symbol size and may have started it in another (from the screen)
//! before this channel came up; the first frames sent here then went by
//! unused. So after everything was sent once, the segments a receiver still
//! reports missing, although it has taken in all that was sent, get repair
//! symbols until they are complete.

use std::collections::VecDeque;
use std::io;

use crate::fec::SegmentEncoder;
use crate::feedback::Feedback;
use crate::frame::{self, Frame};
use crate::sender::{SegmentSource, SessionLayout};

/// Appends one record to a message for a channel that carries bytes: its
/// length (`u32`, little endian), then the record. A record is a frame in
/// binary (§3) or any other code as its text.
pub fn pack(out: &mut Vec<u8>, record: &[u8]) {
    out.extend_from_slice(&(record.len() as u32).to_le_bytes());
    out.extend_from_slice(record);
}

/// The records of a message (see [`pack`]). Stops at the first one that
/// does not fit into what is left.
pub fn unpack(mut message: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        let (len, rest) = message.split_first_chunk::<4>()?;
        let record = rest.get(..u32::from_le_bytes(*len) as usize)?;
        message = &rest[record.len()..];
        Some(record)
    })
}

/// What a record holds.
pub enum Record<'a> {
    Frame(Frame),
    /// A code other than a frame (a notice, feedback, a link code).
    Text(&'a str),
    Unreadable,
}

impl<'a> Record<'a> {
    pub fn parse(record: &'a [u8]) -> Self {
        // Every text code starts with "QS" and a letter where a frame has
        // its version number.
        if record.starts_with(&frame::MAGIC) && record.get(2) == Some(&frame::VERSION) {
            return Frame::decode(record).map_or(Record::Unreadable, Record::Frame);
        }
        std::str::from_utf8(record).map_or(Record::Unreadable, Record::Text)
    }
}

struct Seg {
    k: u64,
    /// Position in the segment's endless symbol stream.
    next_j: u64,
    /// Symbols still to send in the current round.
    quota: u64,
    needed: bool,
    queued: bool,
}

pub struct DirectSender<S> {
    layout: SessionLayout,
    source: S,
    /// Meta first, then the body segments.
    segments: Vec<Seg>,
    queue: VecDeque<u32>,
    current: Option<(u32, SegmentEncoder)>,
    emitted: u64,
}

impl<S: SegmentSource> DirectSender<S> {
    /// `feedback`: what the receiver is already known to have.
    pub fn new(layout: SessionLayout, source: S, feedback: Option<&Feedback>) -> Self {
        let segments: Vec<Seg> = (0..=layout.seg_count())
            .map(|i| {
                let k = layout.k(i) as u64;
                Seg {
                    k,
                    next_j: 0,
                    quota: k,
                    needed: feedback.is_none_or(|f| f.needs(i)),
                    queued: false,
                }
            })
            .collect();
        let mut s = DirectSender {
            layout,
            source,
            segments,
            queue: VecDeque::new(),
            current: None,
            emitted: 0,
        };
        for i in 0..s.segments.len() {
            if s.segments[i].needed {
                s.enqueue(i as u32);
            }
        }
        s
    }

    pub fn layout(&self) -> &SessionLayout {
        &self.layout
    }

    /// Frames handed out so far.
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    fn enqueue(&mut self, index: u32) {
        let seg = &mut self.segments[index as usize];
        if !seg.queued {
            seg.queued = true;
            self.queue.push_back(index);
        }
    }

    /// Takes the receiver's report into account. `settled`: the receiver had
    /// taken in everything sent so far when it wrote this, so whatever it
    /// still lacks will not complete by itself.
    pub fn apply_feedback(&mut self, feedback: &Feedback, settled: bool) {
        if feedback.session_id != self.layout.session_id {
            return;
        }
        for i in 0..self.segments.len() {
            let needed = feedback.needs(i as u32);
            let seg = &mut self.segments[i];
            if needed && !seg.needed {
                // It had this segment and lost it again (it failed
                // verification): everything of it is wanted once more.
                seg.next_j = 0;
                seg.quota = seg.k;
                seg.needed = true;
                self.enqueue(i as u32);
            }
            self.segments[i].needed = needed;
        }
        if settled {
            self.send_more();
        }
    }

    /// Gives every segment that is still needed, and was sent as far as
    /// planned, some repair symbols. For when the receiver is known (or, after
    /// a long silence, assumed) to have taken in everything sent.
    pub fn send_more(&mut self) {
        for i in 0..self.segments.len() {
            let seg = &mut self.segments[i];
            if seg.needed && !seg.queued {
                seg.quota = (seg.k / 4).max(4);
                self.enqueue(i as u32);
            }
        }
    }

    /// The next frame, or `None` when everything was sent as far as planned
    /// (more may follow after the next feedback).
    pub fn next_frame(&mut self) -> io::Result<Option<Frame>> {
        loop {
            let Some(&index) = self.queue.front() else {
                return Ok(None);
            };
            let seg = &mut self.segments[index as usize];
            if !seg.needed || seg.quota == 0 {
                seg.queued = false;
                self.queue.pop_front();
                continue;
            }
            let j = seg.next_j;
            seg.next_j += 1;
            seg.quota -= 1;
            if self.current.as_ref().is_none_or(|(i, _)| *i != index) {
                let data = self.source.segment(index)?;
                if data.len() != self.layout.seg_len(index) as usize {
                    return Err(io::Error::other(format!(
                        "segment {index} has unexpected length"
                    )));
                }
                self.current = Some((index, SegmentEncoder::new(&data, self.layout.symbol_size)));
            }
            let (esi, symbol) = self.current.as_ref().unwrap().1.symbol(j);
            self.emitted += 1;
            return Ok(Some(Frame {
                header: self.layout.header(index, esi),
                symbol,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receiver::{Event, Receiver};
    use crate::schedule::ScheduleConfig;
    use crate::sender::{MemorySource, Sender};

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

    const SHIFT: u8 = 14;

    fn setup(symbol_size: usize) -> (SessionLayout, MemorySource, Vec<u8>) {
        let body = noise(100_000, 3);
        let meta = noise(300, 4);
        let layout = SessionLayout {
            session_id: 9,
            flags: 0,
            seg_shift: SHIFT,
            meta_len: meta.len() as u32,
            body_len: body.len() as u64,
            symbol_size,
        };
        let source = MemorySource {
            meta,
            body: body.clone(),
            seg_shift: SHIFT,
        };
        (layout, source, body)
    }

    /// Feeds frames until the sender has nothing more; returns how many.
    fn drain(tx: &mut DirectSender<MemorySource>, rx: &mut Receiver, body: &mut [u8]) -> u64 {
        let mut n = 0;
        while let Some(frame) = tx.next_frame().unwrap() {
            n += 1;
            for ev in rx.push(frame) {
                if let Event::Completed { index, data } = ev
                    && index > 0
                {
                    let at = (index as usize - 1) << SHIFT;
                    body[at..at + data.len()].copy_from_slice(&data);
                }
            }
        }
        n
    }

    #[test]
    fn records_survive_packing() {
        let (layout, source, _) = setup(1000);
        let mut tx = DirectSender::new(layout, source, None);
        let frame = tx.next_frame().unwrap().unwrap();
        let mut message = Vec::new();
        pack(&mut message, &frame.encode());
        pack(&mut message, b"QSC1-ABC");
        pack(&mut message, &[0xff, 0xfe]);
        let records: Vec<&[u8]> = unpack(&message).collect();
        assert_eq!(records.len(), 3);
        assert!(matches!(Record::parse(records[0]), Record::Frame(f) if f == frame));
        assert!(matches!(
            Record::parse(records[1]),
            Record::Text("QSC1-ABC")
        ));
        assert!(matches!(Record::parse(records[2]), Record::Unreadable));
        // A message cut short gives what is whole.
        assert_eq!(unpack(&message[..message.len() - 1]).count(), 2);
    }

    #[test]
    fn every_source_symbol_once_and_nothing_else() {
        let (layout, source, body) = setup(1000);
        let mut tx = DirectSender::new(layout, source, None);
        let mut rx = Receiver::new();
        let mut got = vec![0u8; body.len()];
        let sent = drain(&mut tx, &mut rx, &mut got);
        assert!(rx.is_complete());
        assert_eq!(got, body);
        // 300 B of meta is one symbol; 100 kB in 16 KiB segments of 1 kB symbols.
        let expected: u64 = (0..=layout.seg_count()).map(|i| layout.k(i) as u64).sum();
        assert_eq!(sent, expected);
        assert_eq!(tx.emitted(), expected);
        // Nothing follows by itself, and nothing after "all here" either.
        let done = rx.feedback(true).unwrap();
        tx.apply_feedback(&done, true);
        assert!(tx.next_frame().unwrap().is_none());
    }

    #[test]
    fn leaves_out_what_the_receiver_has() {
        let (layout, source, _) = setup(1000);
        let mut rx = Receiver::new();
        let mut first = DirectSender::new(layout, source, None);
        // The receiver takes in meta and the first two body segments.
        while rx.completed_count() < 3 {
            rx.push(first.next_frame().unwrap().unwrap());
        }
        let feedback = rx.feedback(false).unwrap();
        let (_, source, body) = setup(1000);
        let mut tx = DirectSender::new(layout, source, Some(&feedback));
        let mut got = vec![0u8; body.len()];
        let sent = drain(&mut tx, &mut rx, &mut got);
        assert!(rx.is_complete());
        let rest: u64 = (3..=layout.seg_count()).map(|i| layout.k(i) as u64).sum();
        assert_eq!(sent, rest);
    }

    #[test]
    fn a_segment_begun_in_another_size_is_finished_with_repair_symbols() {
        let (layout, source, body) = setup(1000);
        // The screen has brought a good part of segment 1 in small symbols.
        let (narrow_layout, narrow_source, _) = setup(100);
        let quiet = ScheduleConfig {
            meta_interval: 0,
            ..ScheduleConfig::default()
        };
        let mut narrow = Sender::new(narrow_layout, narrow_source, quiet, Some(&[1]));
        let mut rx = Receiver::new();
        let mut seen = 0;
        while seen < 60 {
            let f = narrow.next_frame().unwrap();
            if f.header.seg_index == 1 {
                seen += 1;
                rx.push(f);
            }
        }
        let mut tx = DirectSender::new(layout, source, None);
        let mut got = vec![0u8; body.len()];
        drain(&mut tx, &mut rx, &mut got);
        // The first frames of segment 1 went by unused: it is still open.
        assert!(!rx.is_done(1));
        assert_eq!(rx.missing(), vec![1]);
        // Feedback written before everything sent was taken in changes nothing.
        let feedback = rx.feedback(false).unwrap();
        tx.apply_feedback(&feedback, false);
        assert!(tx.next_frame().unwrap().is_none());
        let mut rounds = 0;
        while !rx.is_complete() {
            rounds += 1;
            assert!(rounds < 10, "never completes");
            let feedback = rx.feedback(false).unwrap();
            tx.apply_feedback(&feedback, true);
            assert!(drain(&mut tx, &mut rx, &mut got) > 0);
        }
        assert_eq!(got, body);
    }

    #[test]
    fn a_segment_the_receiver_lost_is_sent_again_from_the_start() {
        let (layout, source, body) = setup(1000);
        let mut tx = DirectSender::new(layout, source, None);
        let mut rx = Receiver::new();
        let mut got = vec![0u8; body.len()];
        drain(&mut tx, &mut rx, &mut got);
        let all = rx.feedback(false).unwrap();
        tx.apply_feedback(&all, true);
        // Segment 2 failed verification at the receiver.
        rx.reset(2);
        got[1 << SHIFT..2 << SHIFT].fill(0);
        let feedback = rx.feedback(false).unwrap();
        tx.apply_feedback(&feedback, false);
        let sent = drain(&mut tx, &mut rx, &mut got);
        assert_eq!(sent, layout.k(2) as u64);
        assert!(rx.is_complete());
        assert_eq!(got, body);
    }
}
