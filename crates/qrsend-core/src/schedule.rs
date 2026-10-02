//! Reference send schedule (docs/PROTOCOL.md §4.1). Receivers never rely on it.

use crate::frame::META_INDEX;

#[derive(Debug, Clone)]
pub struct ScheduleConfig {
    /// Extra repair symbols per pass, as a fraction of `K`.
    pub redundancy: f64,
    /// Segments interleaved symbol by symbol.
    pub window: usize,
    /// Every n-th frame carries a meta symbol (0 disables interleaving).
    pub meta_interval: u64,
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        ScheduleConfig {
            redundancy: 0.10,
            window: 8,
            meta_interval: 10,
        }
    }
}

/// Which symbol of which segment to send next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub seg_index: u32,
    /// Position in the segment's endless symbol stream (see `SegmentEncoder::symbol`).
    pub j: u64,
}

/// Interleaved meta symbols use their own ESI range so they never repeat
/// the symbols meta gets in the regular rotation.
const INTERLEAVE_BASE: u64 = 0x80_0000;

pub struct Scheduler {
    config: ScheduleConfig,
    /// (segment index, symbols per pass)
    segments: Vec<(u32, u64)>,
    frame_no: u64,
    meta_j: u64,
    pass: u64,
    window_start: usize,
    round: u64,
    pos: usize,
}

pub fn symbols_per_pass(k: u32, redundancy: f64) -> u64 {
    k as u64 + (k as f64 * redundancy).ceil() as u64 + 2
}

impl Scheduler {
    /// `segments` lists `(body segment index, K)` to send. Meta (with
    /// `meta_k` source symbols) is part of every pass and also interleaved.
    pub fn new(config: ScheduleConfig, meta_k: u32, segments: Vec<(u32, u32)>) -> Self {
        let segments = std::iter::once((META_INDEX, meta_k))
            .chain(segments.into_iter().filter(|&(i, _)| i != META_INDEX))
            .map(|(i, k)| (i, symbols_per_pass(k, config.redundancy)))
            .collect();
        Scheduler {
            config,
            segments,
            frame_no: 0,
            meta_j: 0,
            pass: 0,
            window_start: 0,
            round: 0,
            pos: 0,
        }
    }

    pub fn pass(&self) -> u64 {
        self.pass
    }

    /// Frames in one full pass (body symbols plus interleaved meta symbols).
    pub fn frames_per_pass(&self) -> u64 {
        let body: u64 = self.segments.iter().map(|&(_, n)| n).sum();
        match self.config.meta_interval {
            0 | 1 => body,
            m => body + body / (m - 1),
        }
    }

    fn meta(&mut self) -> Slot {
        let j = INTERLEAVE_BASE + self.meta_j;
        self.meta_j += 1;
        Slot {
            seg_index: META_INDEX,
            j,
        }
    }

    pub fn next_slot(&mut self) -> Slot {
        self.frame_no += 1;
        let m = self.config.meta_interval;
        if m > 1 && self.frame_no.is_multiple_of(m) {
            return self.meta();
        }
        let w = self.config.window.max(1);
        loop {
            let end = (self.window_start + w).min(self.segments.len());
            let window = &self.segments[self.window_start..end];
            while self.pos < window.len() {
                let (seg_index, n) = window[self.pos];
                self.pos += 1;
                if self.round < n {
                    return Slot {
                        seg_index,
                        j: self.pass * n + self.round,
                    };
                }
            }
            self.pos = 0;
            self.round += 1;
            let longest = window.iter().map(|&(_, n)| n).max().unwrap_or(0);
            if self.round >= longest {
                self.round = 0;
                self.window_start = end;
                if self.window_start >= self.segments.len() {
                    self.window_start = 0;
                    self.pass += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn first_pass_covers_every_symbol_once() {
        let cfg = ScheduleConfig {
            redundancy: 0.1,
            window: 3,
            meta_interval: 5,
        };
        let segs: Vec<(u32, u32)> = (1..=7).map(|i| (i, 10 + i)).collect();
        let mut s = Scheduler::new(cfg.clone(), 2, segs.clone());
        let mut seen: HashMap<u32, Vec<u64>> = HashMap::new();
        let mut metas = 0;
        let mut frames = 0;
        loop {
            let slot = s.next_slot();
            if s.pass() != 0 {
                break;
            }
            frames += 1;
            if slot.seg_index == 0 && slot.j >= INTERLEAVE_BASE {
                metas += 1;
            } else {
                seen.entry(slot.seg_index).or_default().push(slot.j);
            }
        }
        assert!(metas > 0);
        for (i, k) in std::iter::once((0, 2)).chain(segs) {
            let mut js = seen.remove(&i).unwrap();
            js.sort();
            let n = symbols_per_pass(k, cfg.redundancy);
            assert_eq!(js, (0..n).collect::<Vec<_>>(), "segment {i}");
        }
        assert!(s.frames_per_pass().abs_diff(frames) <= 1);
    }

    #[test]
    fn second_pass_sends_fresh_symbols() {
        let cfg = ScheduleConfig {
            meta_interval: 0,
            window: 1,
            ..Default::default()
        };
        let mut s = Scheduler::new(cfg, 1, vec![(1, 4)]);
        let (nm, n) = (symbols_per_pass(1, 0.1), symbols_per_pass(4, 0.1));
        let body: Vec<u64> = (0..(nm + n) * 2)
            .map(|_| s.next_slot())
            .filter(|s| s.seg_index == 1)
            .map(|s| s.j)
            .collect();
        assert_eq!(body, (0..n * 2).collect::<Vec<_>>());
    }

    #[test]
    fn meta_only_when_no_body() {
        let mut s = Scheduler::new(ScheduleConfig::default(), 1, vec![]);
        assert_eq!(s.next_slot(), Slot { seg_index: 0, j: 0 });
        assert_eq!(s.next_slot(), Slot { seg_index: 0, j: 1 });
        assert_eq!(s.frames_per_pass(), symbols_per_pass(1, 0.1));
    }
}
