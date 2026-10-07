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

struct Seg {
    index: u32,
    /// Symbols per pass.
    n: u64,
    /// Position in the segment's endless symbol stream.
    next_j: u64,
    /// Symbols still to send before the schedule moves on.
    quota: u64,
    /// False once the receiver reported it has this segment.
    needed: bool,
}

pub struct Scheduler {
    config: ScheduleConfig,
    /// Meta first, then the body segments in order.
    segments: Vec<Seg>,
    frame_no: u64,
    meta_j: u64,
    pass: u64,
    /// Positions (into `segments`) interleaved at the moment.
    window: Vec<usize>,
    /// First position after the current window.
    next_start: usize,
    pos: usize,
    /// The receiver is answering: stay on a window until it is acknowledged.
    live: bool,
    /// Extra rounds granted to the current window while waiting for that.
    extensions: u32,
}

pub fn symbols_per_pass(k: u32, redundancy: f64) -> u64 {
    k as u64 + (k as f64 * redundancy).ceil() as u64 + 2
}

/// How often a window is extended before the schedule moves on regardless
/// (each extension is a tenth of a pass), so that a receiver which never
/// completes a segment cannot hold everything else up.
const MAX_EXTENSIONS: u32 = 40;

impl Scheduler {
    /// `segments` lists `(body segment index, K)` to send. Meta (with
    /// `meta_k` source symbols) is part of every pass and also interleaved.
    pub fn new(config: ScheduleConfig, meta_k: u32, segments: Vec<(u32, u32)>) -> Self {
        let segments = std::iter::once((META_INDEX, meta_k))
            .chain(segments.into_iter().filter(|&(i, _)| i != META_INDEX))
            .map(|(index, k)| {
                let n = symbols_per_pass(k, config.redundancy);
                Seg {
                    index,
                    n,
                    next_j: 0,
                    quota: n,
                    needed: true,
                }
            })
            .collect();
        let mut s = Scheduler {
            config,
            segments,
            frame_no: 0,
            meta_j: 0,
            pass: 0,
            window: Vec::new(),
            next_start: 0,
            pos: 0,
            live: false,
            extensions: 0,
        };
        s.fill_window();
        s
    }

    pub fn pass(&self) -> u64 {
        self.pass
    }

    /// Frames in one full pass over what is still needed (body symbols plus
    /// interleaved meta symbols).
    pub fn frames_per_pass(&self) -> u64 {
        let body: u64 = self.segments.iter().filter(|s| s.needed).map(|s| s.n).sum();
        match self.config.meta_interval {
            m if m > 1 && self.segments[0].needed => body + body / (m - 1),
            _ => body,
        }
    }

    /// Applies what the receiver reported: segments it has are left out, and
    /// segments it (again) lacks are sent. From now on a window is repeated
    /// until the receiver has it (see `forget_receiver`).
    pub fn set_needed(&mut self, needed: impl Fn(u32) -> bool) {
        self.live = true;
        for s in &mut self.segments {
            let now = needed(s.index);
            if now && !s.needed {
                s.quota = s.n;
            }
            s.needed = now;
        }
    }

    /// Whether to stay on a window until the receiver has it. Turned off
    /// while the receiver is not heard from: what it reported last still
    /// holds, but nothing tells any more when a window is complete.
    pub fn wait_for_receiver(&mut self, on: bool) {
        self.live = on;
    }

    /// The receiver stopped answering: send everything again, pass by pass.
    pub fn forget_receiver(&mut self) {
        self.live = false;
        for s in &mut self.segments {
            if !s.needed {
                s.needed = true;
                s.quota = s.n;
            }
        }
    }

    fn fill_window(&mut self) -> bool {
        let w = self.config.window.max(1);
        self.window.clear();
        self.pos = 0;
        self.extensions = 0;
        let mut i = self.next_start;
        while i < self.segments.len() && self.window.len() < w {
            if self.segments[i].needed {
                self.window.push(i);
            }
            i += 1;
        }
        self.next_start = i;
        !self.window.is_empty()
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
        if m > 1 && self.frame_no.is_multiple_of(m) && self.segments[0].needed {
            return self.meta();
        }
        if !self.segments.iter().any(|s| s.needed) {
            // Nothing is known to be missing (the receiver is still checking
            // what it has): meta symbols are small and never wrong.
            return self.meta();
        }
        loop {
            while self.pos < self.window.len() {
                let seg = &mut self.segments[self.window[self.pos]];
                self.pos += 1;
                if seg.needed && seg.quota > 0 {
                    seg.quota -= 1;
                    seg.next_j += 1;
                    return Slot {
                        seg_index: seg.index,
                        j: seg.next_j - 1,
                    };
                }
            }
            self.pos = 0;
            let open = |s: &Seg| s.needed && s.quota > 0;
            if self.window.iter().any(|&i| open(&self.segments[i])) {
                continue;
            }
            // The window has had its share. With a receiver that answers,
            // keep feeding the segments it still lacks instead of moving on:
            // they complete now rather than a whole pass later.
            let waiting = self.window.iter().any(|&i| self.segments[i].needed);
            if self.live && waiting && self.extensions < MAX_EXTENSIONS {
                self.extensions += 1;
                for &i in &self.window {
                    let seg = &mut self.segments[i];
                    if seg.needed {
                        seg.quota = (seg.n / 10).max(2);
                    }
                }
                continue;
            }
            if !self.fill_window() {
                self.pass += 1;
                for seg in &mut self.segments {
                    seg.quota = seg.n;
                }
                self.next_start = 0;
                self.fill_window();
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
    fn acknowledged_segments_are_left_out() {
        let cfg = ScheduleConfig {
            meta_interval: 0,
            window: 2,
            ..Default::default()
        };
        let mut s = Scheduler::new(cfg, 1, (1..=4).map(|i| (i, 10)).collect());
        // The receiver has meta and segments 1 and 3.
        s.set_needed(|i| i == 2 || i == 4);
        let seen: Vec<u32> = (0..20).map(|_| s.next_slot().seg_index).collect();
        assert!(seen.iter().all(|&i| i == 2 || i == 4), "{seen:?}");
        assert_eq!(s.frames_per_pass(), 2 * symbols_per_pass(10, 0.1));
        // It lost segment 1 again (failed verification): fresh symbols.
        s.set_needed(|i| i == 1);
        let slots: Vec<Slot> = (0..5).map(|_| s.next_slot()).collect();
        assert!(slots.iter().all(|s| s.seg_index == 1), "{slots:?}");
        let js: Vec<u64> = slots.iter().map(|s| s.j).collect();
        assert!(js.windows(2).all(|w| w[1] == w[0] + 1), "{js:?}");
    }

    #[test]
    fn a_window_waits_for_a_receiver_that_answers() {
        let cfg = ScheduleConfig {
            meta_interval: 0,
            window: 2,
            ..Default::default()
        };
        let n = symbols_per_pass(10, 0.1);
        let segs: Vec<(u32, u32)> = (1..=4).map(|i| (i, 10)).collect();

        // Nobody answers: after its share the schedule moves to the next window.
        let mut s = Scheduler::new(cfg.clone(), 1, segs.clone());
        let first: Vec<u32> = (0..3 * n).map(|_| s.next_slot().seg_index).collect();
        assert!(first.contains(&2) && first.contains(&3), "{first:?}");

        // The receiver answers but still lacks segment 1: it keeps getting
        // new symbols, and the next window starts once it is acknowledged.
        let mut s = Scheduler::new(cfg, 1, segs);
        s.set_needed(|i| i >= 1);
        let slots: Vec<Slot> = (0..3 * n).map(|_| s.next_slot()).collect();
        assert!(slots.iter().all(|s| s.seg_index <= 2), "{slots:?}");
        let mut js: Vec<u64> = slots
            .iter()
            .filter(|s| s.seg_index == 1)
            .map(|s| s.j)
            .collect();
        let count = js.len();
        js.dedup();
        assert_eq!(js.len(), count);
        s.set_needed(|i| i >= 3);
        assert!((0..n).all(|_| s.next_slot().seg_index >= 3));

        // It falls silent: what it has is still left out, but the schedule no
        // longer waits for it.
        s.wait_for_receiver(false);
        let quiet: Vec<u32> = (0..3 * n).map(|_| s.next_slot().seg_index).collect();
        assert!(quiet.iter().all(|&i| i >= 3), "{quiet:?}");
        assert!(quiet.contains(&3) && quiet.contains(&4), "{quiet:?}");

        // It stays silent: everything is sent again.
        s.forget_receiver();
        let all: Vec<u32> = (0..6 * n).map(|_| s.next_slot().seg_index).collect();
        assert!((0..=4).all(|i| all.contains(&i)), "{all:?}");
    }

    #[test]
    fn nothing_missing_keeps_the_stream_alive() {
        let mut s = Scheduler::new(ScheduleConfig::default(), 1, vec![(1, 4)]);
        s.set_needed(|_| false);
        assert!((0..10).all(|_| s.next_slot().seg_index == 0));
    }

    #[test]
    fn meta_only_when_no_body() {
        let mut s = Scheduler::new(ScheduleConfig::default(), 1, vec![]);
        assert_eq!(s.next_slot(), Slot { seg_index: 0, j: 0 });
        assert_eq!(s.next_slot(), Slot { seg_index: 0, j: 1 });
        assert_eq!(s.frames_per_pass(), symbols_per_pass(1, 0.1));
    }
}
