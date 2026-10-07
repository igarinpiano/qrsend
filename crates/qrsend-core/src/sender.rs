//! Turns a session's segments into an endless stream of frames.

use std::collections::VecDeque;
use std::io;

use crate::fec::{SegmentEncoder, source_symbol_count};
use crate::feedback::Feedback;
use crate::frame::{Frame, FrameHeader, META_INDEX};
use crate::schedule::{ScheduleConfig, Scheduler};

/// Random access to the bytes of each segment (0 = meta).
pub trait SegmentSource {
    fn segment(&mut self, index: u32) -> io::Result<Vec<u8>>;
}

/// Static description of a session as seen on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLayout {
    pub session_id: u32,
    pub flags: u8,
    pub seg_shift: u8,
    pub meta_len: u32,
    pub body_len: u64,
    pub symbol_size: usize,
}

impl SessionLayout {
    pub fn seg_count(&self) -> u32 {
        self.body_len.div_ceil(1u64 << self.seg_shift) as u32
    }

    pub fn seg_len(&self, index: u32) -> u32 {
        if index == META_INDEX {
            return self.meta_len;
        }
        let nominal = 1u64 << self.seg_shift;
        let start = (index as u64 - 1) * nominal;
        (self.body_len - start).min(nominal) as u32
    }

    pub fn k(&self, index: u32) -> u32 {
        source_symbol_count(self.seg_len(index), self.symbol_size)
    }

    fn header(&self, seg_index: u32, esi: u32) -> FrameHeader {
        FrameHeader {
            flags: self.flags,
            session_id: self.session_id,
            seg_shift: self.seg_shift,
            seg_count: self.seg_count(),
            seg_index,
            seg_len: self.seg_len(seg_index),
            esi,
        }
    }
}

pub struct Sender<S> {
    layout: SessionLayout,
    source: S,
    scheduler: Scheduler,
    cache: VecDeque<(u32, SegmentEncoder)>,
    cache_size: usize,
    /// The latest feedback from the receiver, while it is answering.
    feedback: Option<Feedback>,
}

impl<S: SegmentSource> Sender<S> {
    /// `only` restricts body segments (resume); meta is always sent.
    pub fn new(
        layout: SessionLayout,
        source: S,
        config: ScheduleConfig,
        only: Option<&[u32]>,
    ) -> Self {
        let segments = (1..=layout.seg_count())
            .filter(|i| only.is_none_or(|o| o.contains(i)))
            .map(|i| (i, layout.k(i)))
            .collect();
        let cache_size = config.window + 2;
        let meta_k = layout.k(META_INDEX);
        Sender {
            layout,
            source,
            scheduler: Scheduler::new(config, meta_k, segments),
            cache: VecDeque::new(),
            cache_size,
            feedback: None,
        }
    }

    /// Takes what the receiver reports into account: from now on only what it
    /// still lacks is sent. Returns false for feedback about another session.
    pub fn apply_feedback(&mut self, feedback: Feedback) -> bool {
        if feedback.session_id != self.layout.session_id {
            return false;
        }
        if self.feedback.as_ref() != Some(&feedback) {
            self.scheduler.set_needed(|i| feedback.needs(i));
            self.feedback = Some(feedback);
        }
        self.scheduler.wait_for_receiver(true);
        true
    }

    /// The receiver has not been heard from for a moment (its code is out of
    /// view): keep leaving out what it has, but stop waiting for its answers.
    pub fn receiver_quiet(&mut self) {
        self.scheduler.wait_for_receiver(false);
    }

    /// The receiver has not been heard from for a long time: assume nothing
    /// about what it has.
    pub fn forget_receiver(&mut self) {
        if self.feedback.take().is_some() {
            self.scheduler.forget_receiver();
        }
    }

    /// Sends the body from its end towards its start (see
    /// `Scheduler::set_backwards`).
    pub fn set_backwards(&mut self, backwards: bool) {
        self.scheduler.set_backwards(backwards);
    }

    /// The latest feedback, unless the receiver was forgotten.
    pub fn feedback(&self) -> Option<&Feedback> {
        self.feedback.as_ref()
    }

    /// Source symbols of the whole session (meta included), i.e. the codes a
    /// receiver needs when none is lost.
    pub fn total_symbols(&self) -> u64 {
        (0..=self.layout.seg_count())
            .map(|i| self.layout.k(i) as u64)
            .sum()
    }

    pub fn layout(&self) -> &SessionLayout {
        &self.layout
    }

    pub fn pass(&self) -> u64 {
        self.scheduler.pass()
    }

    pub fn frames_per_pass(&self) -> u64 {
        self.scheduler.frames_per_pass()
    }

    fn encoder(&mut self, index: u32) -> io::Result<&SegmentEncoder> {
        if let Some(pos) = self.cache.iter().position(|(i, _)| *i == index) {
            let entry = self.cache.remove(pos).unwrap();
            self.cache.push_back(entry);
        } else {
            let data = self.source.segment(index)?;
            if data.len() != self.layout.seg_len(index) as usize {
                return Err(io::Error::other(format!(
                    "segment {index} has unexpected length"
                )));
            }
            let enc = SegmentEncoder::new(&data, self.layout.symbol_size);
            if self.cache.len() >= self.cache_size {
                self.cache.pop_front();
            }
            self.cache.push_back((index, enc));
        }
        Ok(&self.cache.back().unwrap().1)
    }

    pub fn next_frame(&mut self) -> io::Result<Frame> {
        let slot = self.scheduler.next_slot();
        let (esi, symbol) = self.encoder(slot.seg_index)?.symbol(slot.j);
        Ok(Frame {
            header: self.layout.header(slot.seg_index, esi),
            symbol,
        })
    }
}

/// In-memory segment source (tests, small transfers, WASM).
pub struct MemorySource {
    pub meta: Vec<u8>,
    pub body: Vec<u8>,
    pub seg_shift: u8,
}

impl SegmentSource for MemorySource {
    fn segment(&mut self, index: u32) -> io::Result<Vec<u8>> {
        if index == META_INDEX {
            return Ok(self.meta.clone());
        }
        let nominal = 1usize << self.seg_shift;
        let start = (index as usize - 1) * nominal;
        let end = (start + nominal).min(self.body.len());
        self.body
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| io::Error::other("segment out of range"))
    }
}
