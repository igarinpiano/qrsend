//! Ways to show the QR stream: a window, the terminal, or exported files.

pub mod export;
pub mod terminal;
#[cfg(feature = "window")]
pub mod window;

use std::time::Instant;

use anyhow::{Context, Result};
use qrsend_core::qr::{self, QrMatrix, QrParams};
use qrsend_core::sender::Sender;

use crate::spool::FileSource;
use crate::util;

/// Quiet zone around each code, in modules (the QR spec asks for 4).
pub const QUIET: usize = 4;

/// The sender plus QR rendering and throughput bookkeeping.
pub struct FrameStream {
    sender: Sender<FileSource>,
    pub params: QrParams,
    pub frames: u64,
    started: Instant,
}

impl FrameStream {
    pub fn new(sender: Sender<FileSource>, params: QrParams) -> Self {
        FrameStream {
            sender,
            params,
            frames: 0,
            started: Instant::now(),
        }
    }

    pub fn next_matrix(&mut self) -> Result<QrMatrix> {
        let frame = self.sender.next_frame()?;
        self.frames += 1;
        let text = frame.to_qr_text();
        qr::render(&text, self.params).with_context(|| {
            format!(
                "frame {} ({} bytes, {} chars, segment {}, esi {})",
                self.frames,
                frame.encoded_len(),
                text.len(),
                frame.header.seg_index,
                frame.header.esi
            )
        })
    }

    pub fn frames_per_pass(&self) -> u64 {
        self.sender.frames_per_pass()
    }

    pub fn pass(&self) -> u64 {
        self.sender.pass()
    }

    pub fn status(&self, fps: f64, grid: usize) -> String {
        let per_pass = self.frames_per_pass();
        let into_pass = self.frames % per_pass.max(1);
        let symbol = self.sender.layout().symbol_size as f64;
        let rate = symbol * fps * (grid * grid) as f64;
        format!(
            "pass {} · {:.0}% · {} frames · {:.1} fps × {} · ~{}/s · {}",
            self.pass() + 1,
            into_pass as f64 * 100.0 / per_pass.max(1) as f64,
            self.frames,
            fps,
            grid * grid,
            util::human_bytes(rate as u64),
            util::human_duration(self.started.elapsed().as_secs_f64()),
        )
    }
}

/// Lays out `codes` (up to grid×grid) into a `w×h` greyscale/RGB-agnostic
/// buffer through `put(x, y, dark)`; returns the module size used.
pub fn layout_grid(
    codes: &[QrMatrix],
    grid: usize,
    w: usize,
    h: usize,
    mut fill: impl FnMut(usize, usize, usize, usize),
) -> usize {
    let Some(first) = codes.first() else { return 0 };
    let cell_w = w / grid;
    let cell_h = h / grid;
    let span = first.width + 2 * QUIET;
    let scale = (cell_w.min(cell_h) / span).max(1);
    let side = span * scale;
    for (n, code) in codes.iter().enumerate() {
        let (col, row) = (n % grid, n / grid);
        let ox = col * cell_w + cell_w.saturating_sub(side) / 2 + QUIET * scale;
        let oy = row * cell_h + cell_h.saturating_sub(side) / 2 + QUIET * scale;
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
