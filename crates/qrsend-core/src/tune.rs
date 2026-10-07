//! Finds the fastest way to show codes to a receiver that answers
//! (docs/PROTOCOL.md §11.4). Nothing here is part of the wire format.
//!
//! The sender can turn two things: how many pictures it shows per second and
//! how many codes a picture holds. What the best setting is depends on the
//! receiver's camera, the distance, the light and the screen, none of which
//! the sender can see. It can see the result, though: the receiver's feedback
//! counts the distinct codes it has read. So the sender tries a change,
//! watches for a few seconds how fast that count grows, keeps the change if
//! it grew faster and takes it back if not.

/// What the sender shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setting {
    /// Pictures per second.
    pub fps: f64,
    /// Index into the layouts the screen offers, from fewest codes to most.
    pub level: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Faster,
    Denser,
    Slower,
    Sparser,
}

const MOVES: [Move; 4] = [Move::Faster, Move::Denser, Move::Slower, Move::Sparser];

pub const MIN_FPS: f64 = 2.0;
pub const MAX_FPS: f64 = 30.0;
const FPS_STEP: f64 = 1.3;
/// After a change, what the receiver reports still belongs to the old
/// setting for a while (codes on their way, the feedback's own delay).
const SETTLE_S: f64 = 1.0;
/// How long a setting is watched before it is judged.
const MEASURE_S: f64 = 2.5;
/// A change has to bring this much more to be kept: less is noise.
const GAIN: f64 = 1.05;
/// Feedback missing for this long: what was being measured is thrown away.
const GAP_S: f64 = 1.5;
/// Evaluations a move that did not help is left alone, at first and at most.
const REST: u32 = 3;
const MAX_REST: u32 = 24;

struct Sample {
    at: f64,
    shown: u64,
    received: u64,
}

pub struct Tuner {
    /// Codes per picture at each level, ascending.
    levels: Vec<u32>,
    setting: Setting,
    /// When the current setting was applied.
    since: f64,
    start: Option<Sample>,
    last_at: f64,
    /// Codes per second the receiver took in at the setting before the trial.
    baseline: Option<f64>,
    /// The move being tried and the setting to return to.
    trial: Option<(Move, Setting)>,
    /// Per move: evaluations left to rest, and how long the next rest is.
    rest: [(u32, u32); 4],
    next_move: usize,
    /// Share of the shown codes the receiver read, at the last evaluation.
    ratio: f64,
}

impl Tuner {
    /// `levels`: codes per picture of every layout the screen offers, from
    /// fewest to most. Starts at `setting`.
    pub fn new(levels: Vec<u32>, setting: Setting) -> Self {
        assert!(!levels.is_empty());
        let setting = Setting {
            fps: setting.fps.clamp(MIN_FPS, MAX_FPS),
            level: setting.level.min(levels.len() - 1),
        };
        Tuner {
            levels,
            setting,
            since: f64::NEG_INFINITY,
            start: None,
            last_at: f64::NEG_INFINITY,
            baseline: None,
            trial: None,
            rest: [(0, REST); 4],
            next_move: 0,
            ratio: 1.0,
        }
    }

    pub fn levels(&self) -> &[u32] {
        &self.levels
    }

    pub fn setting(&self) -> Setting {
        self.setting
    }

    /// Share of the shown codes the receiver read when last measured.
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    /// The person at the screen changed the setting: carry on from there.
    pub fn set(&mut self, now: f64, setting: Setting) {
        self.setting = Setting {
            fps: setting.fps.clamp(MIN_FPS, MAX_FPS),
            level: setting.level.min(self.levels.len() - 1),
        };
        self.since = now;
        self.start = None;
        self.baseline = None;
        self.trial = None;
    }

    fn apply(&self, m: Move) -> Option<Setting> {
        let Setting { fps, level } = self.setting;
        let next = match m {
            Move::Faster => Setting {
                fps: (fps * FPS_STEP).min(MAX_FPS),
                level,
            },
            Move::Slower => Setting {
                fps: (fps / FPS_STEP).max(MIN_FPS),
                level,
            },
            Move::Denser => Setting {
                fps,
                level: (level + 1).min(self.levels.len() - 1),
            },
            Move::Sparser => Setting {
                fps,
                level: level.saturating_sub(1),
            },
        };
        (next != self.setting).then_some(next)
    }

    /// The next move worth trying. While the receiver reads most of what is
    /// shown there is room for more; while it misses most, less may bring
    /// more. Moves the other way are not tried: they cost and cannot gain.
    fn pick(&mut self) -> Option<(Move, Setting)> {
        let up = self.ratio >= 0.5;
        for i in 0..MOVES.len() {
            let at = (self.next_move + i) % MOVES.len();
            let m = MOVES[at];
            if matches!(m, Move::Faster | Move::Denser) != up || self.rest[at].0 > 0 {
                continue;
            }
            if let Some(next) = self.apply(m) {
                self.next_move = at + 1;
                return Some((m, next));
            }
        }
        None
    }

    /// Takes a look at the transfer: `shown` codes were put on the screen so
    /// far and the receiver's latest feedback counts `received` distinct
    /// ones, at `now` (seconds, from any fixed start). Returns the setting to
    /// switch to, if it changes.
    pub fn observe(&mut self, now: f64, shown: u64, received: u64) -> Option<Setting> {
        let gap = now - self.last_at;
        self.last_at = now;
        if gap > GAP_S {
            // The feedback was out of view: nothing can be said about that time.
            self.start = None;
        }
        if now - self.since < SETTLE_S {
            return None;
        }
        let Some(start) = &self.start else {
            self.start = Some(Sample {
                at: now,
                shown,
                received,
            });
            return None;
        };
        let elapsed = now - start.at;
        if elapsed < MEASURE_S {
            return None;
        }
        let read = received.saturating_sub(start.received) as f64;
        let rate = read / elapsed;
        self.ratio = (read / shown.saturating_sub(start.shown).max(1) as f64).min(1.0);
        self.start = None;
        for r in &mut self.rest {
            r.0 = r.0.saturating_sub(1);
        }

        if let Some((m, back)) = self.trial.take() {
            let before = self.baseline.unwrap_or(0.0);
            let at = MOVES.iter().position(|&x| x == m).unwrap();
            // (When nothing got through before and nothing does now, less is
            // still the way to go.)
            let nothing = before <= 0.0 && rate <= 0.0 && matches!(m, Move::Slower | Move::Sparser);
            if rate > before * GAIN || nothing {
                // It helped: keep it, and such moves are worth trying again.
                self.rest[at] = (0, REST);
                self.baseline = Some(rate);
            } else {
                // It did not: back, and leave this move alone for a while
                // (longer each time it fails).
                let (_, pause) = self.rest[at];
                self.rest[at] = (pause, (pause * 2).min(MAX_REST));
                self.setting = back;
                self.since = now;
                return Some(back);
            }
        } else {
            self.baseline = Some(rate);
        }

        let (m, next) = self.pick()?;
        self.trial = Some((m, self.setting));
        self.setting = next;
        self.since = now;
        Some(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A receiver whose camera takes `reads` pictures a second and can make
    /// out the codes of layouts up to `sharp`; denser ones only in part.
    struct Camera {
        reads: f64,
        sharp: usize,
    }

    impl Camera {
        /// Codes read per second at a setting.
        fn rate(&self, levels: &[u32], s: Setting) -> f64 {
            let pictures = s.fps.min(self.reads);
            let legible = match s.level {
                l if l <= self.sharp => 1.0,
                l if l == self.sharp + 1 => 0.3,
                _ => 0.0,
            };
            pictures * levels[s.level] as f64 * legible
        }
    }

    /// Runs a transfer for `seconds`; returns the tuner and the rate over
    /// the last third (trials included: they are part of the price).
    fn run(
        camera: &Camera,
        levels: &[u32],
        start: Setting,
        seconds: f64,
        jitter: f64,
    ) -> (Tuner, f64) {
        let mut tuner = Tuner::new(levels.to_vec(), start);
        let (mut shown, mut received) = (0.0f64, 0.0f64);
        let mut noise = 0x2545_F491_4F6C_DD1Du64;
        let step = 0.25;
        let mut now = 0.0;
        let mut late = 0.0;
        while now < seconds {
            let s = tuner.setting();
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            let wobble = 1.0 + jitter * ((noise % 2001) as f64 / 1000.0 - 1.0);
            shown += s.fps * levels[s.level] as f64 * step;
            received += camera.rate(levels, s) * step * wobble;
            if now >= seconds * 2.0 / 3.0 {
                late += camera.rate(levels, s) * step;
            }
            now += step;
            tuner.observe(now, shown as u64, received as u64);
        }
        (tuner, late / (seconds / 3.0))
    }

    const LEVELS: [u32; 7] = [1, 2, 4, 6, 9, 12, 16];

    #[test]
    fn climbs_to_what_the_camera_can_take() {
        let camera = Camera {
            reads: 14.0,
            sharp: 4,
        };
        let start = Setting {
            fps: 10.0,
            level: 0,
        };
        let (tuner, rate) = run(&camera, &LEVELS, start, 120.0, 0.0);
        let best = 14.0 * 9.0;
        assert!(
            rate >= best * 0.85,
            "{rate} of {best} at {:?}",
            tuner.setting()
        );
        // Thirteen times what it started with.
        assert!(rate > camera.rate(&LEVELS, start) * 10.0);
    }

    #[test]
    fn backs_off_from_a_setting_that_is_too_much() {
        let camera = Camera {
            reads: 8.0,
            sharp: 1,
        };
        // Far too dense and too fast: almost nothing gets through.
        let start = Setting {
            fps: 30.0,
            level: 6,
        };
        assert_eq!(camera.rate(&LEVELS, start), 0.0);
        let (tuner, rate) = run(&camera, &LEVELS, start, 150.0, 0.0);
        let best = 8.0 * 2.0;
        assert!(
            rate >= best * 0.85,
            "{rate} of {best} at {:?}",
            tuner.setting()
        );
    }

    #[test]
    fn noise_does_not_send_it_astray() {
        let camera = Camera {
            reads: 12.0,
            sharp: 3,
        };
        let start = Setting {
            fps: 10.0,
            level: 1,
        };
        let (tuner, rate) = run(&camera, &LEVELS, start, 180.0, 0.03);
        let best = 12.0 * 6.0;
        assert!(
            rate >= best * 0.75,
            "{rate} of {best} at {:?}",
            tuner.setting()
        );
    }

    #[test]
    fn waits_while_the_feedback_is_out_of_view() {
        let mut tuner = Tuner::new(
            LEVELS.to_vec(),
            Setting {
                fps: 10.0,
                level: 0,
            },
        );
        // A look every two seconds is no basis for anything.
        for i in 0..20 {
            assert_eq!(tuner.observe(i as f64 * 2.0, i * 20, i * 20), None);
        }
        // A setting chosen by hand is where it carries on from.
        tuner.set(
            40.0,
            Setting {
                fps: 99.0,
                level: 99,
            },
        );
        assert_eq!(
            tuner.setting(),
            Setting {
                fps: MAX_FPS,
                level: 6
            }
        );
    }
}
