//! The way back from the receiver to a sending `qrsend` (two-way transfer,
//! docs/PROTOCOL.md §11): feedback codes, read off the receiver's screen by
//! a camera or as lines of text from whatever carries bytes. What they say
//! goes to the part that shows the codes ([`crate::display::FrameStream`]),
//! which then sends only what is still missing and ends by itself.
//!
//! The codes are advice without a signature, as everywhere else: they decide
//! what is sent next, never whether something has arrived.

use std::io::BufRead;
use std::path::PathBuf;
use std::thread;

use anyhow::Result;
use crossbeam_channel::{Receiver, bounded, unbounded};
use qrsend_core::feedback::{self, Feedback};

use crate::decode;
use crate::input::{Input, Picture};
use crate::log;

/// Where the receiver's feedback codes come from.
pub enum Source {
    /// Pictures: a camera pointed at the receiver's screen.
    Pictures(Input),
    /// Lines of text ("-": standard input), as `qrsend recv --feedback-to`
    /// writes them.
    Text(PathBuf),
}

/// Reads feedback codes from `source` in the background. An input that
/// fails (no camera, no ffmpeg) is reported once on the terminal; the
/// transfer goes on one-way.
pub fn listen(source: Source) -> Receiver<Feedback> {
    let (tx, rx) = unbounded();
    match source {
        Source::Text(path) => {
            thread::spawn(move || {
                let lines = |reader: &mut dyn BufRead| {
                    // (A damaged line is a code not read, no more.)
                    for line in reader.split(b'\n').map_while(|l| l.ok()) {
                        let line = String::from_utf8_lossy(&line);
                        if let Ok(f) = Feedback::decode(line.trim())
                            && tx.send(f).is_err()
                        {
                            return;
                        }
                    }
                };
                if path.as_os_str() == "-" {
                    lines(&mut std::io::stdin().lock());
                } else {
                    match std::fs::File::open(&path) {
                        Ok(file) => lines(&mut std::io::BufReader::new(file)),
                        Err(e) => {
                            eprintln!("No feedback is read: cannot open {}: {e}", path.display())
                        }
                    }
                }
                log::line("feedback", || "the way back ended".into());
            });
        }
        Source::Pictures(input) => {
            let (pictures_tx, pictures) = bounded::<Picture>(2);
            thread::spawn(move || {
                let result: Result<()> = input.produce(pictures_tx);
                if let Err(e) = result {
                    eprintln!("No feedback is read: {e:#}");
                    log::line("feedback", || "the camera gave nothing".into());
                }
            });
            thread::spawn(move || {
                let mut said_size = false;
                while let Ok(picture) = pictures.recv() {
                    if !said_size {
                        said_size = true;
                        log::line("feedback", || {
                            format!(
                                "camera picture={}x{}",
                                picture.luma.width, picture.luma.height
                            )
                        });
                    }
                    // (The receiver's screen shows its one code, large:
                    // nothing of what reading dense grids takes is needed.)
                    for text in decode::detect(&picture.luma) {
                        if text.starts_with(feedback::PREFIX)
                            && let Ok(f) = Feedback::decode(&text)
                            && tx.send(f).is_err()
                        {
                            return;
                        }
                    }
                }
            });
        }
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use qrsend_core::qr;
    use std::time::Duration;

    fn feedback(seq: u32, complete: bool) -> Feedback {
        Feedback {
            session_id: 0x1234_5678,
            seq,
            complete,
            truncated: false,
            frames: 40,
            remaining_symbols: 7,
            symbol_size: 300,
            missing: vec![(2, 3)],
            camera: Default::default(),
        }
    }

    /// A file of this test's own, removed when the test is over.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            Scratch(
                std::env::temp_dir().join(format!("qrsend-twoway-{}-{name}", std::process::id())),
            )
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn lines_of_text_are_read_and_damaged_ones_passed_over() {
        let scratch = Scratch::new("back.txt");
        let path = scratch.0.clone();
        let text = format!(
            "{}\nnot a code\n\n{}X\n{}\n",
            feedback(1, false).encode(),
            feedback(2, false).encode(),
            feedback(3, true).encode()
        );
        std::fs::write(&path, text).unwrap();
        let rx = listen(Source::Text(path));
        let got: Vec<Feedback> = rx.iter().collect();
        assert_eq!(got.first(), Some(&feedback(1, false)));
        assert_eq!(got.last(), Some(&feedback(3, true)));
        assert_eq!(got.len(), 2);
    }

    /// A feedback code as a camera sees it on the receiver's screen.
    #[test]
    fn a_code_in_a_picture_is_read() {
        let scratch = Scratch::new("screen.png");
        let code = feedback(9, false).encode();
        let matrix = qr::render_text(&code).unwrap();
        let (scale, quiet) = (6, 4);
        let side = (matrix.width + 2 * quiet) * scale;
        let mut picture =
            image::GrayImage::from_pixel(side as u32, side as u32, image::Luma([255]));
        for y in 0..matrix.width {
            for x in 0..matrix.width {
                if matrix.dark(x, y) {
                    for (dx, dy) in (0..scale).flat_map(|dx| (0..scale).map(move |dy| (dx, dy))) {
                        picture.put_pixel(
                            ((x + quiet) * scale + dx) as u32,
                            ((y + quiet) * scale + dy) as u32,
                            image::Luma([0]),
                        );
                    }
                }
            }
        }
        let path = scratch.0.clone();
        picture.save(&path).unwrap();
        let rx = listen(Source::Pictures(Input::Images(vec![path])));
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(20)).ok(),
            Some(feedback(9, false))
        );
    }
}
