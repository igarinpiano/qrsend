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

/// Says when something that recurs in the stream (a meta symbol, a notice,
/// an offer to connect) takes the place of a data code: every so many codes
/// on average, but never at a fixed distance.
///
/// A fixed distance locks such codes to one place in the picture. With three
/// codes per picture (color codes) and one in six being a notice, every
/// notice was in the first of the three colors, and every meta symbol in the
/// third; a camera that made out only the second color never learned what
/// the transfer was, nor that a connection was on offer. The same happens
/// with grids, to a receiver that cannot see one corner. So the distance
/// varies by one either way, which over time visits every place.
#[derive(Debug, Clone)]
pub struct Recurring {
    since: u64,
    gap: u64,
    state: u32,
}

impl Recurring {
    /// `first`: codes to pass before the first turn (0: the very next one).
    pub fn new(first: u64) -> Self {
        Recurring {
            since: 0,
            gap: first,
            state: 0x9E37_79B9,
        }
    }

    /// Call once per code. True when this code is the recurring one's turn;
    /// `every` is the distance to the next turn, give or take one.
    pub fn due(&mut self, every: u64) -> bool {
        if self.since < self.gap {
            self.since += 1;
            return false;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        // One code of the distance is this turn itself.
        let jitter = (self.state % 3) as u64;
        self.gap = (every.max(2) - 2 + jitter).max(1);
        self.since = 0;
        true
    }

    /// The next turn comes at once (something new is to be said).
    pub fn soon(&mut self) {
        self.gap = 0;
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
    /// When a meta symbol is put in between.
    meta_turn: Recurring,
    meta_j: u64,
    pass: u64,
    /// The order segments are gone through in a pass (positions into
    /// `segments`): meta first, then the body from the front or from the back.
    order: Vec<usize>,
    backwards: bool,
    /// Positions (into `segments`) interleaved at the moment.
    window: Vec<usize>,
    /// First index into `order` after the current window.
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

/// One code in this many is a meta symbol while the file list is urgent (see
/// `Scheduler::meta_every`).
const URGENT_META_EVERY: u64 = 3;

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
            .collect::<Vec<Seg>>();
        let meta_turn = Recurring::new(config.meta_interval.saturating_sub(1));
        let mut s = Scheduler {
            config,
            order: (0..segments.len()).collect(),
            backwards: false,
            segments,
            frame_no: 0,
            meta_turn,
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

    /// Goes through the body from the last segment to the first (or back to
    /// the usual order). Two channels sending the same transfer take opposite
    /// ends, so that each brings something the other has not sent yet.
    ///
    /// Backwards, one segment is sent at a time instead of a window of them:
    /// this is the slower channel of two, and preparing a segment for it
    /// (thousands of small symbols) must not hold up the faster one.
    pub fn set_backwards(&mut self, backwards: bool) {
        if backwards == self.backwards {
            return;
        }
        self.backwards = backwards;
        let body = 1..self.segments.len();
        self.order = if backwards {
            std::iter::once(0).chain(body.rev()).collect()
        } else {
            std::iter::once(0).chain(body).collect()
        };
        self.next_start = 0;
        self.fill_window();
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
        let w = if self.backwards {
            1
        } else {
            self.config.window.max(1)
        };
        self.window.clear();
        self.pos = 0;
        self.extensions = 0;
        let mut i = self.next_start;
        while i < self.order.len() && self.window.len() < w {
            let pos = self.order[i];
            if self.segments[pos].needed {
                self.window.push(pos);
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

    /// How often a meta symbol is put in between at the moment.
    ///
    /// The file list of a large transfer is long (a checksum per segment:
    /// 33 kB for 1 GiB, 28 codes at a usual size), and at one code in ten a
    /// camera that reads a fifth of what is shown waited a minute for it. So
    /// it comes more often while that helps: at the start of the stream, for
    /// as many symbols as a pass holds, and for as long as a receiver that
    /// answers says it lacks it.
    fn meta_every(&self) -> u64 {
        let m = self.config.meta_interval;
        if self.meta_j < self.segments[0].n || self.live {
            m.min(URGENT_META_EVERY)
        } else {
            m
        }
    }

    pub fn next_slot(&mut self) -> Slot {
        self.frame_no += 1;
        let m = self.config.meta_interval;
        if m > 1 && self.meta_turn.due(self.meta_every()) && self.segments[0].needed {
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

    /// However many codes a picture holds, what recurs must not keep to one
    /// place in it: a receiver may be unable to read that place (one color
    /// of three, one corner of a grid).
    #[test]
    fn what_recurs_visits_every_place_in_the_picture() {
        for per_picture in 2..=16u64 {
            // The stream as a sender builds it while an offer is waiting:
            // now and then a notice or an offer, otherwise the schedule,
            // which now and then gives a meta symbol.
            let segs: Vec<(u32, u32)> = (1..=20).map(|i| (i, 200)).collect();
            let mut schedule = Scheduler::new(ScheduleConfig::default(), 1, segs);
            let mut extras = Recurring::new(0);
            let places = per_picture as usize;
            let (mut meta, mut extra, mut data) =
                (vec![0u32; places], vec![0u32; places], vec![0u32; places]);
            for i in 0..per_picture * 600 {
                let place = (i % per_picture) as usize;
                if extras.due(6) {
                    extra[place] += 1;
                } else if schedule.next_slot().seg_index == META_INDEX {
                    meta[place] += 1;
                } else {
                    data[place] += 1;
                }
            }
            for (what, seen) in [("meta", &meta), ("extras", &extra), ("data", &data)] {
                assert!(
                    seen.iter().all(|&n| n > 0),
                    "{per_picture} codes per picture: {what} never at some place: {seen:?}"
                );
            }
        }
    }

    #[test]
    fn a_long_file_list_comes_often_at_first_and_while_a_receiver_lacks_it() {
        let segs: Vec<(u32, u32)> = (1..=50).map(|i| (i, 800)).collect();
        let metas = |s: &mut Scheduler, codes: usize| {
            (0..codes)
                .filter(|_| s.next_slot().seg_index == META_INDEX)
                .count()
        };
        // 28 symbols of file list, as for a 1 GiB file.
        let mut s = Scheduler::new(ScheduleConfig::default(), 28, segs.clone());
        // Often at first (one in three, plus its place among the segments)…
        let early = metas(&mut s, 90);
        assert!(early >= 30, "{early}");
        // …then one in ten or so, as before…
        metas(&mut s, 2000);
        let later = metas(&mut s, 1000);
        assert!((80..=130).contains(&later), "{later}");
        // …and often again for a receiver that says it lacks it,
        s.set_needed(|_| true);
        let wanted = metas(&mut s, 300);
        assert!(wanted >= 95, "{wanted}");
        // but not at all once it has it.
        s.set_needed(|i| i != META_INDEX);
        assert_eq!(metas(&mut s, 300), 0);
    }

    #[test]
    fn recurring_keeps_its_average_distance() {
        for every in [3u64, 6, 10, 64] {
            let mut r = Recurring::new(0);
            let turns = (0..every * 3000).filter(|_| r.due(every)).count() as f64;
            let average = (every * 3000) as f64 / turns;
            assert!(
                (average - every as f64).abs() < 0.1,
                "every {every}: {average}"
            );
        }
        // Asked to, the next turn comes at once.
        let mut r = Recurring::new(50);
        assert!(!r.due(50));
        r.soon();
        assert!(r.due(50));
    }

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
        // (An estimate: where exactly meta symbols come in between varies.)
        assert!(s.frames_per_pass().abs_diff(frames) <= frames / 20 + 1);
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
    fn backwards_starts_at_the_other_end() {
        let cfg = ScheduleConfig {
            meta_interval: 0,
            window: 2,
            ..Default::default()
        };
        let mut s = Scheduler::new(cfg, 1, (1..=6).map(|i| (i, 10)).collect());
        let n = symbols_per_pass(10, 0.1);
        let first: Vec<u32> = (0..n).map(|_| s.next_slot().seg_index).collect();
        assert!(first.iter().all(|&i| i <= 1), "{first:?}");
        s.set_backwards(true);
        // The last segment first (meta has had its share in this pass), then
        // toward the front, one segment at a time.
        let seen: Vec<u32> = (0..3 * n).map(|_| s.next_slot().seg_index).collect();
        let mut order = seen.clone();
        order.dedup();
        assert_eq!(order, [6, 5, 4], "{seen:?}");
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
