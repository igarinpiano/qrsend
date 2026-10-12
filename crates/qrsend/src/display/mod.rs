//! Ways to show the QR stream: a window, the terminal, or exported files.

#[cfg(feature = "window")]
pub mod answer;
pub mod export;
pub mod terminal;
#[cfg(feature = "window")]
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use qrsend_core::feedback::{self, Feedback, SenderNotice};
use qrsend_core::qr::{self, QUIET, QrMatrix, QrParams};
use qrsend_core::schedule::Recurring;
use qrsend_core::sender::Sender;
use qrsend_core::tune::{Setting, Tuner};

use crate::log;
use crate::net::LinkEvent;
use crate::spool::FileSource;
use crate::util;

/// Set when the program is asked to end while a window shows the codes, by
/// something its loop does not see by itself: Ctrl-C in the terminal the
/// window was started from, quitting the application (macOS).
static STOP: AtomicBool = AtomicBool::new(false);
/// A loop is running that ends when asked through `STOP`.
static WATCHED: AtomicBool = AtomicBool::new(false);

#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub fn stop_asked() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Asks the loop that shows the codes to end, and says whether one will:
/// one is running, and was not asked before.
pub fn ask_stop() -> bool {
    WATCHED.load(Ordering::SeqCst) && !STOP.swap(true, Ordering::SeqCst)
}

/// For as long as this lives, a loop heeds [`ask_stop`].
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub struct Watched(());

#[cfg_attr(not(feature = "window"), allow(dead_code))]
impl Watched {
    pub fn new() -> Watched {
        WATCHED.store(true, Ordering::SeqCst);
        Watched(())
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        WATCHED.store(false, Ordering::SeqCst);
    }
}

/// Largest grid accepted on the command line.
pub const MAX_GRID: usize = 64;

/// How many codes are shown at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridSpec {
    Fixed {
        cols: usize,
        rows: usize,
    },
    /// As many as fit the window or frame at the minimum module size.
    Auto,
}

impl GridSpec {
    pub const ONE: GridSpec = GridSpec::Fixed { cols: 1, rows: 1 };

    /// Columns and rows for codes of `modules` width in a `w × h` pixel area.
    pub fn resolve(self, modules: usize, w: usize, h: usize, scale: usize) -> (usize, usize) {
        match self {
            GridSpec::Fixed { cols, rows } => (cols, rows),
            GridSpec::Auto => (
                qr::grid_fit(modules, w, scale).clamp(1, MAX_GRID),
                qr::grid_fit(modules, h, scale).clamp(1, MAX_GRID),
            ),
        }
    }
}

impl std::str::FromStr for GridSpec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let bad = || {
            format!("invalid grid {s:?} (use N, COLSxROWS or auto; at most {MAX_GRID} per side)")
        };
        if s == "auto" {
            return Ok(GridSpec::Auto);
        }
        let (c, r) = s.split_once(['x', 'X', '×']).unwrap_or((s, s));
        let cols: usize = c.parse().map_err(|_| bad())?;
        let rows: usize = r.parse().map_err(|_| bad())?;
        if !(1..=MAX_GRID).contains(&cols) || !(1..=MAX_GRID).contains(&rows) {
            return Err(bad());
        }
        Ok(GridSpec::Fixed { cols, rows })
    }
}

/// The sender plus QR rendering and throughput bookkeeping.
pub struct FrameStream {
    sender: Sender<FileSource>,
    pub params: QrParams,
    pub frames: u64,
    started: Instant,
    /// Codes besides frames, repeated in the stream: the offer to connect
    /// over the network.
    extras: Vec<String>,
    /// Codes of an offer that changes as connections come and go.
    extras_live: Vec<String>,
    extra_due: Recurring,
    extra_turn: usize,
    link: Option<Receiver<LinkEvent>>,
    link_up: bool,
    finished: bool,
    /// The receiver's feedback codes, read off its screen or however else
    /// they come (two-way transfer, see `crate::twoway`), and the notice
    /// that asks for them, mixed into the stream.
    feedback: Option<Receiver<Feedback>>,
    asking: bool,
    /// The colors the codes are stacked in (bit 0 red, 1 green, 2 blue; 0:
    /// black and white), what was asked for, and what goes on about them
    /// (see [`Colors`]).
    colors: Colors,
    /// Finds the fastest way to show the codes from what the receiver
    /// reports (`--auto-speed`).
    tuner: Option<Tuner>,
    /// When a feedback code last came that way, and whether one ever did.
    heard: Option<Instant>,
    answered: bool,
    /// … and nothing since for long enough to stop waiting for answers.
    quiet: bool,
}

/// While an offer is waiting for a receiver, every so many codes is one of
/// it. So is the notice that asks for feedback, until a receiver answers.
const EXTRA_EVERY: u64 = 6;
/// A receiver that answers knows it is asked: the notice is for newcomers.
const NOTICE_EVERY: u64 = 30;
/// Without a feedback code for this long (the receiver's screen is out of
/// the camera's view), the sender stops waiting for answers; for this long,
/// it assumes nothing any more about what the receiver has (PROTOCOL §11.3).
const QUIET_AFTER: Duration = Duration::from_secs(2);
const FORGOTTEN_AFTER: Duration = Duration::from_secs(10);

impl FrameStream {
    pub fn new(sender: Sender<FileSource>, params: QrParams) -> Self {
        FrameStream {
            sender,
            params,
            frames: 0,
            started: Instant::now(),
            extras: Vec::new(),
            extras_live: Vec::new(),
            extra_due: Recurring::new(EXTRA_EVERY - 1),
            extra_turn: 0,
            link: None,
            link_up: false,
            finished: false,
            feedback: None,
            asking: false,
            colors: Colors::default(),
            tuner: None,
            heard: None,
            answered: false,
            quiet: false,
        }
    }

    /// A feedback code of this transfer came at some point.
    pub fn answered(&self) -> bool {
        self.answered
    }

    /// Asks the receiver for feedback (a notice mixed into the stream) and
    /// goes by what comes back through `feedback`: only what is missing is
    /// sent, and the stream is finished when the receiver has everything.
    pub fn with_feedback(&mut self, feedback: Receiver<Feedback>) {
        self.asking = true;
        self.feedback = Some(feedback);
    }

    /// Stacks three codes in red, green and blue from now on (color codes,
    /// PROTOCOL §2.3). With a receiver that answers, the colors its camera
    /// does not tell apart are given up along the way.
    pub fn in_color(&mut self) {
        self.colors.shown = 0b111;
    }

    /// The colors to draw the next picture in, one code each (bit 0 red, 1
    /// green, 2 blue); none: one code in black and white.
    pub fn colors(&mut self) -> u8 {
        self.poll();
        self.colors_now()
    }

    fn colors_now(&self) -> u8 {
        let asking = self.asking && !self.link_up;
        self.colors
            .now(asking, self.answered, self.started.elapsed())
    }

    /// Lets the receiver's reports choose pictures per second and codes per
    /// picture. `levels`: codes per picture of every layout the screen
    /// offers, fewest first; `dots`: how large a dot comes out in each;
    /// `start`: the setting until the reports say otherwise. Called again
    /// when the screen offers other layouts (the window was resized).
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub fn tune(&mut self, levels: Vec<u32>, dots: Vec<f64>, start: Setting) {
        match &mut self.tuner {
            Some(tuner) if tuner.levels() == levels => {}
            _ => self.tuner = Some(Tuner::new(levels, start).with_scales(dots)),
        }
    }

    /// The setting the tuner is at, if there is one.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub fn tuned(&self) -> Option<Setting> {
        self.tuner.as_ref().map(|t| t.setting())
    }

    /// The person at the screen changed the speed: the tuner goes on from there.
    #[cfg_attr(not(feature = "window"), allow(dead_code))]
    pub fn tuned_by_hand(&mut self, fps: f64) {
        let now = self.started.elapsed().as_secs_f64();
        if let Some(tuner) = &mut self.tuner {
            let level = tuner.setting().level;
            tuner.set(now, Setting { fps, level });
        }
    }

    /// The notice mixed into the stream: that feedback is read, and in which
    /// colors the codes are stacked (a receiver cannot always tell by
    /// looking). None when there is neither to say.
    fn notice(&self) -> Option<String> {
        let colors = self.colors_now();
        (self.asking || colors != 0).then(|| {
            SenderNotice {
                session_id: self.sender.layout().session_id,
                wants_feedback: self.asking,
                hears_sound: false,
                colors,
            }
            .encode()
        })
    }

    /// Mixes an offer to connect over the network into the stream, and
    /// follows what the connection reports.
    pub fn with_link(&mut self, offer: Vec<String>, events: Receiver<LinkEvent>) {
        self.extras = offer;
        self.link = Some(events);
    }

    /// Takes in what came back from the receiver since the last call.
    fn poll(&mut self) {
        self.poll_feedback();
        self.poll_link();
    }

    /// Feedback codes read off the receiver's screen (or however they come).
    fn poll_feedback(&mut self) {
        let Some(codes) = &self.feedback else { return };
        for f in codes.try_iter() {
            let (complete, read, camera) = (f.complete, f.frames, f.camera);
            // (A code of another transfer says nothing about this one.)
            if !self.sender.apply_feedback(f) {
                continue;
            }
            let now = self.started.elapsed();
            // The count of codes read says how well the screen gets through,
            // unless a connection is bringing codes of its own.
            if !complete && !self.link_up {
                if let Some(to) = self.colors.heard(now, read, camera.colors) {
                    log::line("tx", || format!("colors now={to:03b}"));
                }
                if let Some(tuner) = &mut self.tuner
                    && let Some(to) = retune(tuner, now.as_secs_f64(), self.frames, read, &camera)
                {
                    let codes = tuner.levels()[to.level];
                    log::line("tx", || {
                        format!("tuned fps={:.1} codesPerPicture={codes}", to.fps)
                    });
                }
            }
            if self.heard.is_none() {
                log::line("tx", || {
                    format!("feedback heard codesShown={}", self.frames)
                });
            }
            self.heard = Some(Instant::now());
            self.answered = true;
            self.quiet = false;
            if complete && !self.finished {
                log::line("tx", || "the receiver has everything".into());
            }
            self.finished |= complete;
        }
        // While a network connection is up, the reports come through it.
        if self.link_up {
            return;
        }
        match self.heard.map(|at| at.elapsed()) {
            Some(silent) if silent > FORGOTTEN_AFTER => {
                log::line("tx", || format!("feedback lost codesShown={}", self.frames));
                self.heard = None;
                self.sender.forget_receiver();
            }
            Some(silent) if silent > QUIET_AFTER && !self.quiet => {
                self.quiet = true;
                self.sender.receiver_quiet();
            }
            _ => {}
        }
    }

    /// What the network connection reported.
    fn poll_link(&mut self) {
        let Some(events) = &self.link else { return };
        for event in events.try_iter() {
            match event {
                // The connection brings the transfer from its start: the
                // screen goes on from its end, so both bring something.
                LinkEvent::Up => {
                    log::line("tx", || format!("connection up codesShown={}", self.frames));
                    self.link_up = true;
                    self.sender.set_backwards(true);
                }
                LinkEvent::Down => {
                    log::line("tx", || {
                        format!("connection down codesShown={}", self.frames)
                    });
                    self.link_up = false;
                    self.sender.set_backwards(false);
                    // What the receiver said it has still holds, but
                    // nothing tells any more when a window is complete.
                    self.sender.receiver_quiet();
                }
                LinkEvent::Feedback(f) => {
                    if f.complete && !self.finished {
                        log::line("tx", || "the receiver has everything".into());
                    }
                    self.finished |= f.complete;
                    self.sender.apply_feedback(f);
                }
                LinkEvent::Offer(codes) => self.extras_live = codes,
            }
        }
    }

    /// The receiver reported (through the network connection, or by a
    /// feedback code) that it has everything.
    pub fn finished(&mut self) -> bool {
        self.poll();
        self.finished
    }

    /// The next code of the stream as text: a frame, or now and then a part
    /// of the offer.
    pub fn next_text(&mut self) -> Result<String> {
        self.poll();
        self.frames += 1;
        if log::on() {
            log::every("tx", std::time::Duration::from_secs(2), "tx", || {
                format!(
                    "progress codesShown={} connection={}",
                    self.frames,
                    if self.link_up { "up" } else { "none" }
                )
            });
        }
        // Besides frames: the parts of an offer while it waits for a
        // receiver, and the notice that asks for feedback.
        let offers = if self.link_up {
            0
        } else {
            self.extras.len() + self.extras_live.len()
        };
        let notice = self.notice();
        let extras = offers + notice.is_some() as usize;
        if extras > 0 {
            // (Often while something waits for an answer: an offer, or the
            // request for feedback. Now and then once it is only a reminder.)
            let waiting = offers > 0 || (self.asking && self.heard.is_none());
            let every = if waiting { EXTRA_EVERY } else { NOTICE_EVERY };
            // Never at a fixed distance, which would keep these codes to one
            // place in a grid (see `Recurring`).
            if self.extra_due.due(every) {
                self.extra_turn = (self.extra_turn + 1) % extras;
                let turn = self.extra_turn;
                let code = if turn >= offers {
                    notice.as_ref()
                } else if turn < self.extras.len() {
                    self.extras.get(turn)
                } else {
                    self.extras_live.get(turn - self.extras.len())
                };
                if let Some(code) = code {
                    return Ok(code.clone());
                }
            }
        }
        Ok(self.sender.next_frame()?.to_qr_text())
    }

    pub fn next_matrix(&mut self) -> Result<QrMatrix> {
        let text = self.next_text()?;
        qr::render(&text, self.params)
            .with_context(|| format!("code {} ({} characters)", self.frames, text.len()))
    }

    /// The next `n` codes. Rendering (mask selection) dominates the cost, so
    /// larger batches are rendered on all cores.
    pub fn next_matrices(&mut self, n: usize) -> Result<Vec<QrMatrix>> {
        if n < 4 {
            return (0..n).map(|_| self.next_matrix()).collect();
        }
        let texts = (0..n)
            .map(|_| self.next_text())
            .collect::<Result<Vec<_>>>()?;
        let params = self.params;
        let threads = std::thread::available_parallelism()
            .map(|t| t.get())
            .unwrap_or(1)
            .min(n);
        let chunk = n.div_ceil(threads);
        let rendered: Vec<Result<Vec<QrMatrix>, qr::QrError>> = std::thread::scope(|scope| {
            let handles: Vec<_> = texts
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || part.iter().map(|t| qr::render(t, params)).collect())
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("render thread panicked"))
                .collect()
        });
        let mut out = Vec::with_capacity(n);
        for part in rendered {
            out.extend(part?);
        }
        Ok(out)
    }

    pub fn frames_per_pass(&self) -> u64 {
        self.sender.frames_per_pass()
    }

    pub fn pass(&self) -> u64 {
        self.sender.pass()
    }

    pub fn symbol_size(&self) -> usize {
        self.sender.layout().symbol_size
    }

    /// `codes` is the number of codes shown per tick.
    ///
    /// The sender cannot know how much the receiver still needs, so it reports
    /// its own schedule: what is left of the current pass (one pass is enough
    /// for a receiver that misses up to the redundancy), and the nominal speed.
    pub fn status(&self, fps: f64, codes: usize) -> String {
        let per_pass = self.frames_per_pass().max(1);
        let shown = self.frames % per_pass;
        let left = per_pass - shown;
        let per_second = fps * codes as f64;
        let rate = self.symbol_size() as f64 * per_second;
        if self.link_up {
            return format!(
                "sending over the network · the screen adds ~{}/s · running {}",
                util::human_bytes(rate as u64),
                util::human_duration(self.started.elapsed().as_secs_f64())
            );
        }
        format!(
            "pass {} · {} of {} codes, {} left ({}) · {:.1} fps × {} · ~{}/s · running {}",
            self.pass() + 1,
            shown,
            per_pass,
            left,
            util::human_duration(left as f64 / per_second),
            fps,
            codes,
            util::human_bytes(rate as u64),
            util::human_duration(self.started.elapsed().as_secs_f64()),
        )
    }
}

/// Lays out `codes` row by row in a `cols × rows` grid centered in a `w × h`
/// pixel area, calling `fill(x, y, w, h)` for every dark module. Neighboring
/// codes share one quiet zone. Returns the module size used (at least 1).
pub fn layout_grid(
    codes: &[QrMatrix],
    cols: usize,
    rows: usize,
    w: usize,
    h: usize,
    mut fill: impl FnMut(usize, usize, usize, usize),
) -> usize {
    layout_layers(codes, 1, cols, rows, w, h, |_, x, y, cw, ch| {
        fill(x, y, cw, ch)
    })
}

/// [`layout_grid`] with `layers` codes in every place of the grid, one on
/// top of the other (color codes: each goes into a color of its own).
/// `codes` holds them place by place; `fill` is told the layer first.
pub fn layout_layers(
    codes: &[QrMatrix],
    layers: usize,
    cols: usize,
    rows: usize,
    w: usize,
    h: usize,
    mut fill: impl FnMut(usize, usize, usize, usize, usize),
) -> usize {
    let Some(first) = codes.first() else { return 0 };
    let layers = layers.max(1);
    let modules = first.width;
    let (ext_w, ext_h) = (
        qr::grid_extent(modules, cols),
        qr::grid_extent(modules, rows),
    );
    let scale = (w / ext_w).min(h / ext_h).max(1);
    let x0 = w.saturating_sub(ext_w * scale) / 2;
    let y0 = h.saturating_sub(ext_h * scale) / 2;
    let pitch = (modules + QUIET) * scale;
    for (n, code) in codes.iter().take(cols * rows * layers).enumerate() {
        let (place, layer) = (n / layers, n % layers);
        let ox = x0 + QUIET * scale + (place % cols) * pitch;
        let oy = y0 + QUIET * scale + (place / cols) * pitch;
        for y in 0..code.width {
            for x in 0..code.width {
                if code.dark(x, y) {
                    fill(layer, ox + x * scale, oy + y * scale, scale, scale);
                }
            }
        }
    }
    scale
}

/// The colors of `colors` (bit 0 red, 1 green, 2 blue) in the order their
/// codes are stacked.
pub fn channels(colors: u8) -> Vec<usize> {
    (0..3).filter(|c| colors & (1 << c) != 0).collect()
}

/// A picture of `w × h` pixels (one number each, 0x00RRGGBB) with `codes`
/// in a `cols × rows` grid: black on white, or, with `colors`, as many codes
/// stacked in every place as there are colors, each dark where its color is
/// off (PROTOCOL §2.3).
pub fn paint(
    codes: &[QrMatrix],
    colors: u8,
    cols: usize,
    rows: usize,
    w: usize,
    h: usize,
) -> Vec<u32> {
    let mut buf = vec![0x00FF_FFFFu32; w * h];
    let channels = channels(colors);
    // (Black and white: one layer that takes every color away.)
    let off: Vec<u32> = match channels.len() {
        0 => vec![0x00FF_FFFF],
        _ => channels.iter().map(|c| 0xFFu32 << (16 - 8 * c)).collect(),
    };
    layout_layers(codes, off.len(), cols, rows, w, h, |layer, x, y, cw, ch| {
        let keep = !off[layer];
        for row in y..(y + ch).min(h) {
            let end = (x + cw).min(w);
            if x < end {
                for px in &mut buf[row * w + x..row * w + end] {
                    *px &= keep;
                }
            }
        }
    });
    buf
}

/// The layouts a `w × h` area offers codes of `modules` width, for a tuner
/// to choose from: (columns, rows, size of a module in pixels), from one
/// code to as many as fit at `min_scale` pixels per module. Only layouts
/// worth choosing are listed: none that holds fewer codes than another at
/// modules no larger.
#[cfg_attr(not(feature = "window"), allow(dead_code))]
pub fn layouts(modules: usize, w: usize, h: usize, min_scale: usize) -> Vec<(usize, usize, usize)> {
    let (max_cols, max_rows) = GridSpec::Auto.resolve(modules, w, h, min_scale);
    let mut all = Vec::new();
    for cols in 1..=max_cols {
        for rows in 1..=max_rows {
            let scale =
                (w / qr::grid_extent(modules, cols)).min(h / qr::grid_extent(modules, rows));
            if scale >= min_scale.max(1) || (cols, rows) == (1, 1) {
                all.push((cols, rows, scale.max(1)));
            }
        }
    }
    // Most codes first; kept if its modules are larger than those of every
    // layout with more codes.
    all.sort_by_key(|&(c, r, scale)| (std::cmp::Reverse(c * r), std::cmp::Reverse(scale)));
    let mut kept: Vec<(usize, usize, usize)> = Vec::new();
    for layout in all {
        if kept.last().is_none_or(|&(.., scale)| layout.2 > scale) {
            kept.push(layout);
        }
    }
    kept.reverse();
    kept
}

/// One report of the receiver, for the tuner: `shown` codes were put on the
/// screen so far, the receiver has read `read` distinct ones, and its
/// camera says what it makes of them. What the camera says can settle a lot
/// at once (the first time); after that the count of codes read guides the
/// search. Returns the setting to switch to, if it changes.
fn retune(
    tuner: &mut Tuner,
    at: f64,
    shown: u64,
    read: u64,
    camera: &feedback::Camera,
) -> Option<Setting> {
    let (reads, dot) = (
        camera.reads_tenths as f64 / 10.0,
        camera.dot_tenths as f64 / 10.0,
    );
    tuner
        .hint(at, reads, dot)
        .or_else(|| tuner.observe(at, shown, read))
}

/// Which colors color codes are shown in, as the receiver's reports settle
/// it (PROTOCOL §11.4): a camera may read only some of the three, take two
/// for the same, or make out nothing while they are stacked.
#[derive(Debug, Default)]
struct Colors {
    /// What is shown when nothing holds it back (0: black and white, and
    /// nothing here applies). Colors that are given up do not come back.
    shown: u8,
    /// What the reports last suggested instead, and how often in a row.
    suggested: (u8, u32),
    /// No judging before this time (a change takes a moment to show).
    settled: Duration,
    /// Codes the receiver had read, and when that count last went up.
    read: Option<(u64, Duration)>,
}

/// The same suggestion this often in a row: it is followed.
const COLOR_VOTES: u32 = 4;
const COLOR_SETTLE: Duration = Duration::from_secs(3);
/// A receiver asked for feedback that says nothing for this long while
/// colors are shown may not be able to read them at all: black and white,
/// until it is heard.
const COLOR_UNHEARD: Duration = Duration::from_secs(8);
/// Heard, and yet for this long not a code more is read in color: black and
/// white for good.
const COLOR_STALLED: Duration = Duration::from_secs(6);

impl Colors {
    /// The colors to draw in now. `asking`: feedback is read; `answered`:
    /// some has come; `running`: time since the start.
    fn now(&self, asking: bool, answered: bool, running: Duration) -> u8 {
        if asking && !answered && running > COLOR_UNHEARD {
            return 0;
        }
        self.shown
    }

    /// Takes a report into account: the receiver has read `read` codes, and
    /// its camera sees the colors as `seen` says (0: not told). Returns the
    /// colors shown from now on, if they change.
    fn heard(&mut self, now: Duration, read: u64, seen: u8) -> Option<u8> {
        if self.shown == 0 {
            return None;
        }
        // Reports, and yet nothing more is read: these colors do not get
        // through to this camera.
        match self.read {
            Some((count, _)) if read > count => self.read = Some((read, now)),
            Some((_, since)) if now.saturating_sub(since) > COLOR_STALLED => {
                self.shown = 0;
                return Some(0);
            }
            Some(_) => {}
            None => self.read = Some((read, now)),
        }
        if seen == 0 || now < self.settled {
            return None;
        }
        let mut worth = feedback::colors_worth_showing(self.shown, seen);
        if worth.count_ones() < 2 {
            worth = 0;
        }
        if worth == self.shown {
            self.suggested = (worth, 0);
            return None;
        }
        self.suggested = match self.suggested {
            (was, votes) if was == worth => (worth, votes + 1),
            _ => (worth, 1),
        };
        if self.suggested.1 < COLOR_VOTES {
            return None;
        }
        self.shown = worth;
        self.suggested = (worth, 0);
        self.settled = now + COLOR_SETTLE;
        Some(worth)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_spec_parsing() {
        assert_eq!("3".parse(), Ok(GridSpec::Fixed { cols: 3, rows: 3 }));
        assert_eq!("8x4".parse(), Ok(GridSpec::Fixed { cols: 8, rows: 4 }));
        assert_eq!("auto".parse(), Ok(GridSpec::Auto));
        assert!("0".parse::<GridSpec>().is_err());
        assert!("65".parse::<GridSpec>().is_err());
        assert!("axb".parse::<GridSpec>().is_err());
    }

    #[test]
    fn the_layouts_a_window_offers() {
        // From one code to as many as fit, each with smaller dots than the
        // one before: nothing a tuner would never want.
        let offered = layouts(101, 1600, 900, 4);
        assert_eq!(offered.first().map(|&(c, r, _)| (c, r)), Some((1, 1)));
        assert_eq!(offered.last().map(|&(c, r, _)| (c, r)), Some((3, 2)));
        for pair in offered.windows(2) {
            assert!(pair[0].0 * pair[0].1 < pair[1].0 * pair[1].1, "{offered:?}");
            assert!(pair[0].2 > pair[1].2, "{offered:?}");
        }
        assert!(offered.iter().all(|&(.., dot)| dot >= 4));
        // Too small for even one code at that size: one code all the same.
        assert_eq!(layouts(101, 200, 200, 4), [(1, 1, 1)]);
    }

    #[test]
    fn a_receiver_that_reads_everything_is_shown_more() {
        let offered = layouts(101, 1600, 900, 4);
        let levels: Vec<u32> = offered.iter().map(|&(c, r, _)| (c * r) as u32).collect();
        let dots: Vec<f64> = offered.iter().map(|&(.., d)| d as f64).collect();
        let start = Setting {
            fps: 10.0,
            level: 0,
        };
        let none = feedback::Camera::default();
        // Step by step, from the count of codes read alone.
        let mut tuner = Tuner::new(levels.clone(), start).with_scales(dots.clone());
        let (mut shown, mut at) = (0u64, 0.0);
        for _ in 0..600 {
            at += 0.1;
            let now = tuner.setting();
            shown += (now.fps * levels[now.level] as f64 * 0.1) as u64;
            retune(&mut tuner, at, shown, shown, &none);
        }
        let rate = |s: Setting| s.fps * levels[s.level] as f64;
        assert!(
            rate(tuner.setting()) > 3.0 * rate(start),
            "{:?}",
            tuner.setting()
        );
        // At once, from what the receiver's camera says of itself: it reads
        // 25 pictures a second, and a dot is 12 of its pixels wide.
        let mut tuner = Tuner::new(levels.clone(), start).with_scales(dots);
        let camera = feedback::Camera {
            reads_tenths: 250,
            dot_tenths: 120,
            colors: 0,
        };
        let to = retune(&mut tuner, 0.1, 1, 1, &camera).unwrap();
        assert!(to.fps > start.fps && to.level > 0, "{to:?}");
    }

    #[test]
    fn colors_a_camera_cannot_use_are_given_up() {
        let s = Duration::from_secs;
        // Reads all three apart (bits 0–2): they stay.
        let mut colors = Colors {
            shown: 0b111,
            ..Colors::default()
        };
        for i in 0..10 {
            assert_eq!(colors.heard(s(i), 10 * i, 0b000_111), None);
        }
        assert_eq!(colors.now(true, true, s(60)), 0b111);
        // Reads nothing in red: after the same word four times, two colors.
        let mut colors = Colors {
            shown: 0b111,
            ..Colors::default()
        };
        let said: Vec<_> = (0..4)
            .map(|i| colors.heard(s(i), 10 * i, 0b000_110))
            .collect();
        assert_eq!(said, [None, None, None, Some(0b110)]);
        // Takes all three for the same: black and white.
        let mut colors = Colors {
            shown: 0b111,
            ..Colors::default()
        };
        let said: Vec<_> = (0..4)
            .map(|i| colors.heard(s(i), 10 * i, 0b111_111))
            .collect();
        assert_eq!(said.last(), Some(&Some(0)));
        assert_eq!(colors.now(true, true, s(60)), 0);
        // Says nothing of colors, and reads on: nothing changes. Then not a
        // code more for a while: black and white.
        let mut colors = Colors {
            shown: 0b111,
            ..Colors::default()
        };
        assert_eq!(colors.heard(s(1), 5, 0), None);
        assert_eq!(colors.heard(s(5), 9, 0), None);
        assert_eq!(colors.heard(s(10), 9, 0), None);
        assert_eq!(colors.heard(s(12), 9, 0), Some(0));
        // Asked for feedback and never answered: black and white meanwhile.
        let colors = Colors {
            shown: 0b111,
            ..Colors::default()
        };
        assert_eq!(colors.now(true, false, s(5)), 0b111);
        assert_eq!(colors.now(true, false, s(9)), 0);
        assert_eq!(colors.now(false, false, s(9)), 0b111);
    }

    #[test]
    fn color_codes_are_one_code_per_color() {
        use crate::decode;
        use crate::input::LumaFrame;
        let params = QrParams {
            version: 6,
            ec: qr::Ec::L,
        };
        let texts = ["RED 0123456789", "GREEN 0123456789", "BLUE 0123456789"];
        let codes: Vec<QrMatrix> = texts
            .iter()
            .map(|t| qr::render(t, params).unwrap())
            .collect();
        let (w, h) = (360, 360);
        let picture = paint(&codes, 0b111, 1, 1, w, h);
        // Each color of the picture, taken alone, is that color's code.
        for (channel, text) in texts.iter().enumerate() {
            let frame = LumaFrame {
                width: w,
                height: h,
                pixels: picture
                    .iter()
                    .map(|p| (p >> (16 - 8 * channel)) as u8)
                    .collect(),
            };
            assert_eq!(decode::detect(&frame), [*text]);
        }
        // Two colors: the third stays light all over.
        let picture = paint(&codes[..2], 0b101, 1, 1, w, h);
        assert!(picture.iter().all(|p| (p >> 8) as u8 == 255));
    }

    #[test]
    fn auto_grid_fits_the_area() {
        let (cols, rows) = GridSpec::Auto.resolve(177, 1920, 1080, 2);
        assert_eq!((cols, rows), (5, 2));
        assert_eq!(GridSpec::Auto.resolve(177, 100, 100, 2), (1, 1));
    }
}
