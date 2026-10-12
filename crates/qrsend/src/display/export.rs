//! Writes the stream to PNG files or a video instead of a screen.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use image::GrayImage;
use qrsend_core::qr;

use super::{FrameStream, GridSpec, layout_grid};

/// Geometry of exported frames.
#[derive(Debug, Clone, Copy)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Canvas {
    /// `size` fixes the frame; otherwise it is just large enough for the grid
    /// at `scale` pixels per module. Dimensions are even (4:2:0 video).
    pub fn new(
        modules: usize,
        grid: GridSpec,
        size: Option<(usize, usize)>,
        scale: usize,
    ) -> Result<Canvas> {
        let even = |v: usize| (v + 1) & !1;
        match (grid, size) {
            (GridSpec::Auto, None) => bail!("--grid auto needs --size WIDTHxHEIGHT when exporting"),
            (_, Some((w, h))) => {
                let (cols, rows) = grid.resolve(modules, w, h, scale);
                if qr::grid_extent(modules, cols) > w || qr::grid_extent(modules, rows) > h {
                    bail!(
                        "a {cols}×{rows} grid of {modules}-module codes does not fit in {w}×{h} pixels"
                    );
                }
                Ok(Canvas {
                    width: even(w),
                    height: even(h),
                    cols,
                    rows,
                })
            }
            (GridSpec::Fixed { cols, rows }, None) => Ok(Canvas {
                width: even(qr::grid_extent(modules, cols) * scale),
                height: even(qr::grid_extent(modules, rows) * scale),
                cols,
                rows,
            }),
        }
    }

    pub fn codes(&self) -> usize {
        self.cols * self.rows
    }

    fn render(&self, stream: &mut FrameStream) -> Result<Vec<u8>> {
        let codes = stream.next_matrices(self.codes())?;
        let (w, h) = (self.width, self.height);
        let mut px = vec![255u8; w * h];
        layout_grid(&codes, self.cols, self.rows, w, h, |x, y, cw, ch| {
            for row in y..(y + ch).min(h) {
                px[row * w + x..row * w + (x + cw).min(w)].fill(0);
            }
        });
        Ok(px)
    }
}

pub fn png_frames(stream: &mut FrameStream, dir: &Path, count: u64, canvas: Canvas) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    for i in 0..count {
        let name = dir.join(format!("frame-{i:06}.png"));
        let colors = stream.colors();
        if colors == 0 {
            let px = canvas.render(stream)?;
            let img = GrayImage::from_raw(canvas.width as u32, canvas.height as u32, px)
                .expect("buffer size");
            img.save(name)?;
            continue;
        }
        // Color codes: as many codes in every place as there are colors.
        let layers = colors.count_ones() as usize;
        let codes = stream.next_matrices(canvas.codes() * layers)?;
        let (w, h) = (canvas.width, canvas.height);
        let px = super::paint(&codes, colors, canvas.cols, canvas.rows, w, h)
            .into_iter()
            .flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
            .collect();
        let img = image::RgbImage::from_raw(w as u32, h as u32, px).expect("buffer size");
        img.save(name)?;
    }
    Ok(())
}

enum Sink {
    File(BufWriter<File>),
    Ffmpeg(Child),
}

impl Sink {
    fn writer(&mut self) -> &mut dyn Write {
        match self {
            Sink::File(f) => f,
            Sink::Ffmpeg(child) => child.stdin.as_mut().expect("piped stdin"),
        }
    }
}

fn is_y4m(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("y4m"))
}

/// Encoder arguments by container: H.264 for MP4/MOV/MKV, VP9 for WebM.
fn encoder_args(path: &Path, crf: u8) -> Vec<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut args = match ext.as_str() {
        "webm" => s(&["-c:v", "libvpx-vp9", "-b:v", "0", "-row-mt", "1"]),
        _ => s(&[
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-tune",
            "stillimage",
        ]),
    };
    args.extend(s(&["-crf", &crf.to_string(), "-pix_fmt", "yuv420p"]));
    if matches!(ext.as_str(), "mp4" | "mov" | "m4v") {
        args.extend(s(&["-movflags", "+faststart"]));
    }
    args
}

/// Writes a video: `.y4m` natively (uncompressed YUV4MPEG2 4:2:0, also usable
/// as Chrome's fake camera), anything else by piping through ffmpeg.
pub fn video(
    stream: &mut FrameStream,
    path: &Path,
    count: u64,
    canvas: Canvas,
    fps: f64,
    crf: u8,
) -> Result<()> {
    let mut sink = if is_y4m(path) {
        Sink::File(BufWriter::with_capacity(
            1 << 20,
            File::create(path).with_context(|| format!("cannot create {}", path.display()))?,
        ))
    } else {
        let child = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "yuv4mpegpipe", "-i", "-"])
            .args(encoder_args(path, crf))
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    anyhow::anyhow!(
                        "writing {} needs ffmpeg on PATH (https://ffmpeg.org); .y4m is written without it",
                        path.display()
                    )
                } else {
                    e.into()
                }
            })?;
        Sink::Ffmpeg(child)
    };
    let (w, h) = (canvas.width, canvas.height);
    let fps_milli = (fps * 1000.0).round().max(1.0) as u64;
    let chroma = vec![128u8; w * h / 2];
    let result = (|| -> Result<()> {
        let out = sink.writer();
        writeln!(
            out,
            "YUV4MPEG2 W{w} H{h} F{fps_milli}:1000 Ip A1:1 C420jpeg"
        )?;
        for _ in 0..count {
            let px = canvas.render(stream)?;
            out.write_all(b"FRAME\n")?;
            out.write_all(&px)?;
            out.write_all(&chroma)?;
        }
        out.flush()?;
        Ok(())
    })();
    if let Sink::Ffmpeg(mut child) = sink {
        drop(child.stdin.take());
        let status = child.wait()?;
        if !status.success() {
            bail!("ffmpeg failed to encode {}", path.display());
        }
    }
    result
}

/// Without a count, and without a word from the receiver so far, writing
/// stops after this many passes: a way back that brings nothing (a file, a
/// receiver that never started) must not mean writing without end.
pub const UNANSWERED_PASSES: u64 = 3;
/// Whenever a pass's worth of frames has gone out since the receiver last
/// said something, the sender waits this long at most for its word: longer
/// before the first one (the receiver may only just have started), briefly
/// after that (it reports many times a second). A channel like a pipe
/// takes frames far faster than a report comes back; without the wait, a
/// sender would have written its three passes and given the receiver up
/// before the first report could arrive, and would bury a receiver that
/// does answer under frames it no longer needs. It is a pause, not a
/// condition: when nothing comes, sending goes on.
const FIRST_WORD: Duration = Duration::from_secs(2);
const NEXT_WORD: Duration = Duration::from_millis(200);

/// Writes frames as text, one per line ("-" = standard output): `count` of
/// them, or (none) until the receiver reports that it has everything, which
/// takes a way back from it (`--feedback-from`). Returns how many it wrote.
pub fn text(stream: &mut FrameStream, path: &Path, count: Option<u64>) -> Result<u64> {
    let mut out: Box<dyn Write> = if path.as_os_str() == "-" {
        Box::new(BufWriter::new(std::io::stdout().lock()))
    } else {
        Box::new(BufWriter::new(
            File::create(path).with_context(|| format!("cannot create {}", path.display()))?,
        ))
    };
    let mut written = 0;
    let pass = stream.frames_per_pass().max(1);
    // Frames written since the receiver's last report, and the reports so far.
    let (mut since, mut reports) = (0u64, 0u64);
    while count.is_none_or(|n| written < n) && !stream.finished() {
        if count.is_none() {
            if since >= pass {
                let patience = if stream.answered() {
                    NEXT_WORD
                } else {
                    FIRST_WORD
                };
                let until = Instant::now() + patience;
                while stream.reports() == reports && !stream.finished() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(5));
                }
                since = 0;
                if stream.finished() {
                    break;
                }
            }
            let now = stream.reports();
            if now != reports {
                (reports, since) = (now, 0);
            }
            if written >= pass * UNANSWERED_PASSES && !stream.answered() {
                break;
            }
        }
        let line = stream.next_text()?;
        // With a receiver that answers, a line held back in a buffer is a
        // line it cannot answer to.
        let sent = writeln!(out, "{line}").and_then(|()| match count {
            None => out.flush(),
            Some(_) => Ok(()),
        });
        if let Err(e) = sent {
            // A receiver that has everything stops reading, and its last
            // word may still be on its way back.
            let until = Instant::now() + Duration::from_secs(1);
            while count.is_none() && !stream.finished() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            if count.is_none() && stream.finished() {
                return Ok(written);
            }
            return Err(e.into());
        }
        written += 1;
        since += 1;
    }
    out.flush()?;
    Ok(written)
}
