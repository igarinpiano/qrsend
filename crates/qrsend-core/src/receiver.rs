//! Frame collection state machine (transport layer only; no crypto, no I/O).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::fec::SegmentDecoder;
use crate::feedback::{self, Feedback};
use crate::frame::{Frame, META_INDEX};

/// Parameters every frame of a session must agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionParams {
    pub session_id: u32,
    pub flags: u8,
    pub seg_shift: u8,
    pub seg_count: u32,
}

impl SessionParams {
    fn of(frame: &Frame) -> Self {
        let h = &frame.header;
        SessionParams {
            session_id: h.session_id,
            flags: h.flags,
            seg_shift: h.seg_shift,
            seg_count: h.seg_count,
        }
    }

    /// Byte offset of a body segment inside the body.
    pub fn offset(&self, index: u32) -> u64 {
        (index as u64 - 1) << self.seg_shift
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// First accepted frame: the receiver is now bound to this session.
    Locked(SessionParams),
    /// A frame of some other session was seen (reported once per id).
    ForeignSession(u32),
    /// A frame contradicted the locked session parameters.
    Inconsistent,
    /// A segment was reconstructed. Callers verify it and may `reset` it.
    Completed { index: u32, data: Vec<u8> },
}

pub struct Receiver {
    params: Option<SessionParams>,
    expected: Option<u32>,
    done: Vec<bool>,
    seg_lens: HashMap<u32, u32>,
    decoders: HashMap<u32, (SegmentDecoder, u64)>,
    max_decoders: usize,
    tick: u64,
    foreign: Vec<u32>,
    frames: u64,
    useful: u64,
    evicted: u64,
    /// Symbol size of the most recent frame (0 before the first one).
    symbol_size: usize,
    /// The frames read most recently, to tell new ones from repeats (a camera
    /// usually sees every displayed code more than once).
    recent: HashSet<FrameKey>,
    recent_order: VecDeque<FrameKey>,
    /// Per segment: the other symbol size on offer and how many bytes of it
    /// went by unused (see `push`).
    other_size: HashMap<u32, (usize, u64)>,
    /// Bytes taken in per symbol size, to report progress in the size that
    /// carries the transfer.
    size_bytes: HashMap<usize, u64>,
    distinct: u64,
    feedback_seq: u32,
}

/// How many recent frames are remembered to recognize repeats.
const RECENT_FRAMES: usize = 8192;

/// Segment, ESI and symbol size: what makes two frames carry the same data.
type FrameKey = (u32, u32, usize);

/// How far a transfer has come, in terms a person watching it cares about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// Bytes on the wire (meta + body). An estimate until every segment has
    /// been seen at least once: unseen segments count with their nominal size.
    pub total_bytes: u64,
    /// Bytes still to be received, counting partly received segments
    /// proportionally.
    pub remaining_bytes: u64,
    /// Codes the whole transfer takes at the current symbol size, without
    /// counting redundancy (an estimate, like `total_bytes`).
    pub total_symbols: u64,
    /// Codes still needed at the current symbol size (a lower bound: codes
    /// that repeat what is already known do not count).
    pub remaining_symbols: u64,
    /// Payload bytes per code at the moment.
    pub symbol_size: usize,
}

impl Default for Receiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Receiver {
    pub fn new() -> Self {
        Receiver {
            params: None,
            expected: None,
            done: Vec::new(),
            seg_lens: HashMap::new(),
            decoders: HashMap::new(),
            max_decoders: 128,
            tick: 0,
            foreign: Vec::new(),
            frames: 0,
            useful: 0,
            evicted: 0,
            symbol_size: 0,
            recent: HashSet::new(),
            recent_order: VecDeque::new(),
            other_size: HashMap::new(),
            size_bytes: HashMap::new(),
            distinct: 0,
            feedback_seq: 0,
        }
    }

    /// Only accept frames of this session (e.g. when resuming).
    pub fn expect_session(mut self, session_id: u32) -> Self {
        self.expected = Some(session_id);
        self
    }

    /// Cap on partially decoded segments held in memory (LRU eviction).
    pub fn with_max_decoders(mut self, n: usize) -> Self {
        self.max_decoders = n.max(1);
        self
    }

    pub fn params(&self) -> Option<&SessionParams> {
        self.params.as_ref()
    }

    pub fn is_done(&self, index: u32) -> bool {
        self.done.get(index as usize).copied().unwrap_or(false)
    }

    /// Marks a segment as already stored (restored from disk).
    pub fn mark_done(&mut self, index: u32) {
        if let Some(d) = self.done.get_mut(index as usize) {
            *d = true;
        }
        self.decoders.remove(&index);
    }

    /// Forget a completed segment, e.g. after it failed verification.
    pub fn reset(&mut self, index: u32) {
        if let Some(d) = self.done.get_mut(index as usize) {
            *d = false;
        }
        self.drop_decoder(index);
    }

    /// Discards what was collected for a segment. Its frames are no longer
    /// repeats: the same ones are needed again.
    fn drop_decoder(&mut self, index: u32) {
        self.decoders.remove(&index);
        self.other_size.remove(&index);
        self.recent.retain(|key| key.0 != index);
        self.recent_order.retain(|key| key.0 != index);
    }

    /// Locks onto a session without a frame (restoring saved state).
    pub fn restore(&mut self, params: SessionParams) {
        self.params = Some(params);
        self.done = vec![false; params.seg_count as usize + 1];
    }

    pub fn missing(&self) -> Vec<u32> {
        (0..self.done.len() as u32)
            .filter(|&i| !self.done[i as usize])
            .collect()
    }

    pub fn completed_count(&self) -> usize {
        self.done.iter().filter(|&&d| d).count()
    }

    pub fn segment_total(&self) -> usize {
        self.done.len()
    }

    pub fn is_complete(&self) -> bool {
        !self.done.is_empty() && self.done.iter().all(|&d| d)
    }

    /// (frames seen, frames that advanced some segment, decoders evicted)
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.frames, self.useful, self.evicted)
    }

    /// Remaining work, once a session is locked and a frame has been seen.
    pub fn progress(&self) -> Option<Progress> {
        let p = self.params?;
        // Count in the symbol size most of the data arrives in: with several
        // channels at once the latest frame may be of any of them.
        let t = self
            .size_bytes
            .iter()
            .max_by_key(|&(&size, &bytes)| (bytes, size))
            .map_or(self.symbol_size, |(&size, _)| size);
        if t == 0 {
            return None;
        }
        let nominal = 1u64 << p.seg_shift;
        // Meta is small; before its first frame assume it fits one symbol.
        let len = |i: u32| match self.seg_lens.get(&i) {
            Some(&l) => l as u64,
            None if i == META_INDEX => t as u64,
            None => nominal,
        };
        let mut out = Progress {
            symbol_size: t,
            ..Progress::default()
        };
        for i in 0..=p.seg_count {
            let l = len(i);
            let k = l.div_ceil(t as u64).max(1);
            out.total_bytes += l;
            out.total_symbols += k;
            if self.is_done(i) {
                continue;
            }
            // A decoder needs at least one more symbol until it completes.
            let have = match self.decoders.get(&i) {
                Some((d, _)) if d.symbol_size() == t => (d.received() as u64).min(k - 1),
                _ => 0,
            };
            out.remaining_symbols += k - have;
            out.remaining_bytes += l - l * have / k;
        }
        Some(out)
    }

    /// What to tell the sender (None before a session is locked). `complete`
    /// is the caller's verdict: every segment received, verified and stored.
    pub fn feedback(&mut self, complete: bool) -> Option<Feedback> {
        let p = self.params?;
        self.feedback_seq = self.feedback_seq.wrapping_add(1);
        let missing = feedback::ranges(&self.missing());
        Some(Feedback {
            session_id: p.session_id,
            seq: self.feedback_seq,
            complete,
            truncated: missing.len() > feedback::MAX_RANGES,
            frames: self.distinct,
            remaining_symbols: self.progress().map_or(0, |p| p.remaining_symbols),
            symbol_size: self.progress().map_or(0, |p| p.symbol_size as u32),
            missing,
        })
    }

    /// Partially received segments: (index, symbols received, K).
    pub fn partial(&self) -> Vec<(u32, u32, u32)> {
        let mut v: Vec<_> = self
            .decoders
            .iter()
            .map(|(&i, (d, _))| (i, d.received(), d.k()))
            .collect();
        v.sort_unstable();
        v
    }

    /// Feeds one frame; returns what happened (usually nothing).
    pub fn push(&mut self, frame: Frame) -> Vec<Event> {
        self.frames += 1;
        let h = frame.header;
        let mut events = Vec::new();
        match self.params {
            None => {
                if self.expected.is_some_and(|e| e != h.session_id) {
                    return self.foreign(h.session_id);
                }
                let p = SessionParams::of(&frame);
                self.restore(p);
                events.push(Event::Locked(p));
            }
            Some(p) if p.session_id != h.session_id => return self.foreign(h.session_id),
            Some(p) if p != SessionParams::of(&frame) => return vec![Event::Inconsistent],
            Some(_) => {}
        }
        if *self.seg_lens.entry(h.seg_index).or_insert(h.seg_len) != h.seg_len {
            events.push(Event::Inconsistent);
            return events;
        }
        let symbol_size = frame.symbol.len();
        // A repeat of a frame read a moment ago carries nothing new.
        let key = (h.seg_index, h.esi, symbol_size);
        if self.recent.contains(&key) {
            return events;
        }
        // A decoder only combines equal-size symbols, but frames of several
        // sizes may arrive for one segment: another QR density after a
        // resume, or two channels at once (a screen and a network link). The
        // segment is collected in the size that brings the most: frames of
        // another size are passed over until they would have brought more
        // than has been collected, and then the segment starts over in that
        // size. A few strays cost nothing; a faster channel takes over at once.
        if !self.is_done(h.seg_index)
            && let Some((d, _)) = self.decoders.get(&h.seg_index)
            && d.symbol_size() != symbol_size
        {
            let collected = d.received() as u64 * d.symbol_size() as u64;
            let other = self
                .other_size
                .entry(h.seg_index)
                .or_insert((symbol_size, 0));
            if other.0 != symbol_size {
                *other = (symbol_size, 0);
            }
            other.1 += symbol_size as u64;
            if other.1 <= collected {
                return events;
            }
            self.drop_decoder(h.seg_index);
        }
        self.recent.insert(key);
        self.recent_order.push_back(key);
        if self.recent_order.len() > RECENT_FRAMES
            && let Some(old) = self.recent_order.pop_front()
        {
            self.recent.remove(&old);
        }
        self.distinct += 1;
        if self.is_done(h.seg_index) {
            return events;
        }
        self.tick += 1;
        self.symbol_size = symbol_size;
        *self.size_bytes.entry(symbol_size).or_insert(0) += symbol_size as u64;
        if !self.decoders.contains_key(&h.seg_index) && self.decoders.len() >= self.max_decoders {
            self.evict();
        }
        let tick = self.tick;
        let (decoder, last) = self
            .decoders
            .entry(h.seg_index)
            .or_insert_with(|| (SegmentDecoder::new(h.seg_len, symbol_size), tick));
        *last = tick;
        self.useful += 1;
        if let Some(data) = decoder.push(h.esi, &frame.symbol) {
            self.decoders.remove(&h.seg_index);
            self.done[h.seg_index as usize] = true;
            events.push(Event::Completed {
                index: h.seg_index,
                data,
            });
        }
        events
    }

    fn evict(&mut self) {
        // Keep meta: it is small and needed to interpret everything else.
        let victim = self
            .decoders
            .iter()
            .filter(|(i, _)| **i != META_INDEX)
            .min_by_key(|(_, (_, t))| *t)
            .map(|(i, _)| *i);
        if let Some(i) = victim {
            self.drop_decoder(i);
            self.evicted += 1;
        }
    }

    fn foreign(&mut self, id: u32) -> Vec<Event> {
        if self.foreign.contains(&id) {
            return Vec::new();
        }
        self.foreign.push(id);
        vec![Event::ForeignSession(id)]
    }
}
