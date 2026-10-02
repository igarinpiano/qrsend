//! Shows the stream in a native window (largest, densest option).

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use minifb::{Key, KeyRepeat, ScaleMode, Window, WindowOptions};

use super::{FrameStream, layout_grid};

const WHITE: u32 = 0x00FF_FFFF;
const BLACK: u32 = 0x0000_0000;

pub fn run(stream: &mut FrameStream, mut fps: f64, grid: usize) -> Result<()> {
    let opts = WindowOptions {
        resize: true,
        scale_mode: ScaleMode::UpperLeft,
        ..WindowOptions::default()
    };
    let mut window = Window::new("QRSend", 900, 900, opts)
        .context("cannot open a window (try --display terminal)")?;
    window.set_target_fps(120);

    let mut buf: Vec<u32> = Vec::new();
    let mut paused = false;
    let mut next = Instant::now();
    let mut last_title = Instant::now() - Duration::from_secs(2);
    let mut dirty = true;
    let mut codes = Vec::new();

    while window.is_open() && !window.is_key_down(Key::Escape) && !window.is_key_down(Key::Q) {
        if window.is_key_pressed(Key::Space, KeyRepeat::No) {
            paused = !paused;
        }
        if window.is_key_pressed(Key::Up, KeyRepeat::Yes)
            || window.is_key_pressed(Key::Equal, KeyRepeat::Yes)
        {
            fps = (fps * 1.25).min(60.0);
        }
        if window.is_key_pressed(Key::Down, KeyRepeat::Yes)
            || window.is_key_pressed(Key::Minus, KeyRepeat::Yes)
        {
            fps = (fps / 1.25).max(0.5);
        }
        let (w, h) = window.get_size();
        if !paused && Instant::now() >= next {
            codes = (0..grid * grid)
                .map(|_| stream.next_matrix())
                .collect::<Result<Vec<_>>>()?;
            dirty = true;
            next += Duration::from_secs_f64(1.0 / fps);
            if next < Instant::now() {
                next = Instant::now();
            }
        }
        if dirty || buf.len() != w * h {
            buf.clear();
            buf.resize(w * h, WHITE);
            layout_grid(&codes, grid, w, h, |x, y, cw, ch| {
                for row in y..(y + ch).min(h) {
                    let end = (x + cw).min(w);
                    if x < end {
                        buf[row * w + x..row * w + end].fill(BLACK);
                    }
                }
            });
            window.update_with_buffer(&buf, w, h)?;
            dirty = false;
        } else {
            window.update();
        }
        if last_title.elapsed() >= Duration::from_millis(500) {
            let state = if paused { "PAUSED · " } else { "" };
            window.set_title(&format!("QRSend — {state}{}", stream.status(fps, grid)));
            last_title = Instant::now();
        }
    }
    Ok(())
}
