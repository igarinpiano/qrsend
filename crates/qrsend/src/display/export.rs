//! Writes the stream to PNG files or a Y4M video instead of a screen.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};
use image::GrayImage;

use super::{FrameStream, QUIET, layout_grid};

fn canvas(stream: &mut FrameStream, grid: usize, scale: usize) -> Result<(usize, Vec<u8>)> {
    let codes = (0..grid * grid)
        .map(|_| stream.next_matrix())
        .collect::<Result<Vec<_>>>()?;
    let span = (codes[0].width + 2 * QUIET) * scale;
    // Even dimensions keep 4:2:0 video happy.
    let side = (span * grid + 1) & !1;
    let mut px = vec![255u8; side * side];
    layout_grid(&codes, grid, side, side, |x, y, w, h| {
        for row in y..y + h {
            px[row * side + x..row * side + x + w].fill(0);
        }
    });
    Ok((side, px))
}

pub fn png_frames(
    stream: &mut FrameStream,
    dir: &Path,
    count: u64,
    grid: usize,
    scale: usize,
) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    for i in 0..count {
        let (side, px) = canvas(stream, grid, scale)?;
        let img = GrayImage::from_raw(side as u32, side as u32, px).expect("buffer size");
        img.save(dir.join(format!("frame-{i:06}.png")))?;
    }
    Ok(())
}

/// YUV4MPEG2 (4:2:0, neutral chroma): readable by ffmpeg and by Chrome's
/// fake camera (`--use-file-for-fake-video-capture`).
pub fn y4m(
    stream: &mut FrameStream,
    path: &Path,
    count: u64,
    grid: usize,
    scale: usize,
    fps: f64,
) -> Result<()> {
    let mut out = BufWriter::new(
        File::create(path).with_context(|| format!("cannot create {}", path.display()))?,
    );
    let fps_milli = (fps * 1000.0).round().max(1.0) as u64;
    let mut header_written = false;
    for _ in 0..count {
        let (side, px) = canvas(stream, grid, scale)?;
        if !header_written {
            writeln!(
                out,
                "YUV4MPEG2 W{side} H{side} F{fps_milli}:1000 Ip A1:1 C420jpeg"
            )?;
            header_written = true;
        }
        out.write_all(b"FRAME\n")?;
        out.write_all(&px)?;
        out.write_all(&vec![128u8; side * side / 2])?;
    }
    out.flush()?;
    Ok(())
}
