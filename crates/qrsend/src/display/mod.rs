//! Ways to show the QR stream: a window, the terminal, or exported files.

pub mod export;
pub mod terminal;
#[cfg(feature = "window")]
pub mod window;

use std::time::Instant;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use qrsend_core::qr::{self, QUIET, QrMatrix, QrParams};
use qrsend_core::schedule::Recurring;
use qrsend_core::sender::Sender;

use crate::log;
use crate::net::LinkEvent;
use crate::spool::FileSource;
use crate::util;

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
}

/// While an offer is waiting for a receiver, every so many codes is one of it.
const EXTRA_EVERY: u64 = 6;

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
        }
    }

    /// Mixes an offer to connect over the network into the stream, and
    /// follows what the connection reports.
    pub fn with_link(&mut self, offer: Vec<String>, events: Receiver<LinkEvent>) {
        self.extras = offer;
        self.link = Some(events);
    }

    /// Takes in what the network connection reported since the last call.
    fn poll(&mut self) {
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

    /// The receiver reported (through the network connection) that it has
    /// everything.
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
        let extras = self.extras.len() + self.extras_live.len();
        if extras > 0 && !self.link_up {
            // Never at a fixed distance, which would keep the offer to one
            // place in a grid (see `Recurring`).
            if self.extra_due.due(EXTRA_EVERY) {
                self.extra_turn = (self.extra_turn + 1) % extras;
                let code = match self.extra_turn.checked_sub(self.extras.len()) {
                    Some(i) => &self.extras_live[i],
                    None => &self.extras[self.extra_turn],
                };
                return Ok(code.clone());
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
    let Some(first) = codes.first() else { return 0 };
    let modules = first.width;
    let (ext_w, ext_h) = (
        qr::grid_extent(modules, cols),
        qr::grid_extent(modules, rows),
    );
    let scale = (w / ext_w).min(h / ext_h).max(1);
    let x0 = w.saturating_sub(ext_w * scale) / 2;
    let y0 = h.saturating_sub(ext_h * scale) / 2;
    let pitch = (modules + QUIET) * scale;
    for (n, code) in codes.iter().take(cols * rows).enumerate() {
        let ox = x0 + QUIET * scale + (n % cols) * pitch;
        let oy = y0 + QUIET * scale + (n / cols) * pitch;
        for y in 0..code.width {
            for x in 0..code.width {
                if code.dark(x, y) {
                    fill(ox + x * scale, oy + y * scale, scale, scale);
                }
            }
        }
    }
    scale
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
    fn auto_grid_fits_the_area() {
        let (cols, rows) = GridSpec::Auto.resolve(177, 1920, 1080, 2);
        assert_eq!((cols, rows), (5, 2));
        assert_eq!(GridSpec::Auto.resolve(177, 100, 100, 2), (1, 1));
    }
}
