//! Frame collection state machine (transport layer only; no crypto, no I/O).

use std::collections::HashMap;

use crate::fec::SegmentDecoder;
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
        self.decoders.remove(&index);
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
        if self.is_done(h.seg_index) {
            return events;
        }
        self.tick += 1;
        let symbol_size = frame.symbol.len();
        // The symbol size may change between sender runs (e.g. another QR
        // density after a resume); a decoder only combines equal-size symbols.
        if self
            .decoders
            .get(&h.seg_index)
            .is_some_and(|(d, _)| d.symbol_size() != symbol_size)
        {
            self.decoders.remove(&h.seg_index);
        }
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
            self.decoders.remove(&i);
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
