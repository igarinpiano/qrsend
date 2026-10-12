//! Shows the receiver's feedback codes in a window, for the sender's camera
//! to read (two-way transfer, docs/PROTOCOL.md §11): one code, as large as
//! the window allows, drawn anew a few times a second at most.

use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use minifb::{ScaleMode, Window, WindowOptions};
use qrsend_core::qr;

use super::{Watched, layout_grid};

const WHITE: u32 = 0x00FF_FFFF;
const BLACK: u32 = 0x0000_0000;
/// A code stays this long at least: a camera has to get one good look at it.
const SHOWN_FOR: Duration = Duration::from_millis(300);
/// The window is given its events this often at most (the receiving loop
/// comes round far more often than a window needs).
const EVENTS_EVERY: Duration = Duration::from_millis(30);

/// `code` as a picture of `w` × `h` pixels, one number per pixel.
pub fn paint(code: &str, w: usize, h: usize) -> Result<Vec<u32>> {
    let matrix = qr::render_text(code).context("feedback code")?;
    let mut buf = vec![WHITE; w * h];
    layout_grid(&[matrix], 1, 1, w, h, |x, y, cw, ch| {
        for row in y..(y + ch).min(h) {
            let end = (x + cw).min(w);
            if x < end {
                buf[row * w + x..row * w + end].fill(BLACK);
            }
        }
    });
    Ok(buf)
}

pub struct Answer {
    window: Window,
    /// What is shown, since when, and at which size.
    shown: Option<(String, Instant, (usize, usize))>,
    events: Instant,
    _watched: Watched,
}

impl Answer {
    pub fn open() -> Result<Answer> {
        let opts = WindowOptions {
            resize: true,
            scale_mode: ScaleMode::UpperLeft,
            ..WindowOptions::default()
        };
        let mut window = Window::new("QRSend — feedback for the sender's camera", 560, 560, opts)
            .context(
            "cannot open a window for the feedback codes (use --feedback-to instead)",
        )?;
        #[cfg(target_os = "macos")]
        {
            super::window::dock_icon();
            super::window::quit_ends_the_loop();
        }
        // (No waiting in `update`: the loop that calls it has work to do.)
        window.set_target_fps(0);
        Ok(Answer {
            window,
            shown: None,
            events: Instant::now(),
            _watched: Watched::new(),
        })
    }

    /// Whether the code shown has had its time (or none is shown yet).
    pub fn due(&self) -> bool {
        self.shown
            .as_ref()
            .is_none_or(|(_, since, _)| since.elapsed() >= SHOWN_FOR)
    }

    /// Shows `code` from now on (none: what is shown stays), and lets the
    /// window see to its events. False: the window was closed.
    pub fn show(&mut self, code: Option<String>) -> bool {
        if !self.window.is_open() {
            return false;
        }
        let size = self.window.get_size();
        let resized = self.shown.as_ref().is_some_and(|(.., s)| *s != size);
        let code = match (code, &self.shown) {
            (Some(code), _) => Some(code),
            (None, Some((code, ..))) if resized => Some(code.clone()),
            _ => None,
        };
        match code.and_then(|c| paint(&c, size.0, size.1).ok().map(|buf| (c, buf))) {
            Some((code, buf)) => {
                let _ = self.window.update_with_buffer(&buf, size.0, size.1);
                self.shown = Some((code, Instant::now(), size));
                self.events = Instant::now();
            }
            None if self.events.elapsed() >= EVENTS_EVERY => {
                self.window.update();
                self.events = Instant::now();
            }
            None => {}
        }
        true
    }

    /// Shows `code` for `time` (the last word, that everything arrived),
    /// unless the window is closed or the program asked to end before.
    pub fn hold(&mut self, code: String, time: Duration) {
        let until = Instant::now() + time;
        let mut code = Some(code);
        while Instant::now() < until && self.show(code.take()) && !super::stop_asked() {
            std::thread::sleep(EVENTS_EVERY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;
    use crate::input::LumaFrame;

    /// What the window shows is a code a camera reads back as it was.
    #[test]
    fn the_picture_holds_the_code() {
        let code = "QSF1-1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        for (w, h) in [(560, 560), (900, 400), (200, 640)] {
            let buf = paint(code, w, h).unwrap();
            let frame = LumaFrame {
                width: w,
                height: h,
                pixels: buf
                    .iter()
                    .map(|&p| if p == BLACK { 0 } else { 255 })
                    .collect(),
            };
            assert_eq!(decode::detect(&frame), [code], "{w}×{h}");
        }
    }
}
