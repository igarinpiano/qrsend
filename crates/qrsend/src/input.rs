//! Frame sources for the receiver: image files, Y4M video, anything ffmpeg reads.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;

/// One grayscale picture to scan for QR codes.
pub struct LumaFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

/// The color of a picture, kept the way the source gave it and taken apart
/// only when asked.
pub enum Color {
    /// Red, green and blue of each pixel in turn.
    Rgb(Vec<u8>),
    /// The two chroma planes of a YUV picture, each `cw` × `ch`.
    Yuv {
        u: Vec<u8>,
        v: Vec<u8>,
        cw: usize,
        ch: usize,
    },
}

/// One picture from the input: its brightness and, when it has any, its color.
pub struct Picture {
    pub luma: LumaFrame,
    pub color: Option<Color>,
}

impl Picture {
    /// The red, green and blue of the picture as three pictures of their
    /// own: color codes carry one code in each (PROTOCOL §2.3).
    pub fn channels(&self) -> Option<[LumaFrame; 3]> {
        let (w, h) = (self.luma.width, self.luma.height);
        let mut planes = [vec![0u8; w * h], vec![0u8; w * h], vec![0u8; w * h]];
        match self.color.as_ref()? {
            Color::Rgb(rgb) => {
                for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
                    planes[0][i] = px[0];
                    planes[1][i] = px[1];
                    planes[2][i] = px[2];
                }
            }
            Color::Yuv { u, v, cw, ch } => {
                let clamp = |x: i32| (x >> 10).clamp(0, 255) as u8;
                for y in 0..h {
                    let crow = (y * ch / h) * cw;
                    for x in 0..w {
                        let i = y * w + x;
                        let c = crow + x * cw / w;
                        // BT.601, limited range, in 1/1024.
                        let l = 1192 * (self.luma.pixels[i] as i32 - 16) + 512;
                        let (cb, cr) = (u[c] as i32 - 128, v[c] as i32 - 128);
                        planes[0][i] = clamp(l + 1634 * cr);
                        planes[1][i] = clamp(l - 401 * cb - 832 * cr);
                        planes[2][i] = clamp(l + 2066 * cb);
                    }
                }
            }
        }
        Some(planes.map(|pixels| LumaFrame {
            width: w,
            height: h,
            pixels,
        }))
    }
}

pub enum Input {
    Images(Vec<PathBuf>),
    Video(PathBuf),
    /// A live camera, captured through ffmpeg (device name/index as ffmpeg expects it).
    Camera {
        device: String,
        /// Picture size and pictures per second to ask the camera for
        /// (otherwise whatever it gives by default, which is often small).
        size: Option<(usize, usize)>,
        fps: Option<f64>,
    },
}

/// The platform's default camera for `--camera` without a value.
pub fn default_camera() -> &'static str {
    if cfg!(target_os = "macos") {
        "0"
    } else if cfg!(windows) {
        ""
    } else {
        "/dev/video0"
    }
}

/// ffmpeg input arguments for a camera on this platform.
fn camera_input(
    device: &str,
    size: Option<(usize, usize)>,
    fps: Option<f64>,
) -> Result<Vec<String>> {
    let (format, source) = if cfg!(target_os = "macos") {
        ("avfoundation", format!("{device}:none"))
    } else if cfg!(windows) {
        if device.is_empty() {
            bail!(
                "name the camera: --camera \"<device name>\" (list them with `ffmpeg -list_devices true -f dshow -i dummy`)"
            );
        }
        ("dshow", format!("video={device}"))
    } else {
        ("v4l2", device.to_string())
    };
    let mut v = vec!["-f".to_string(), format.to_string()];
    // (macOS refuses to open a camera without a frame rate it supports.)
    let fps = fps.or(cfg!(target_os = "macos").then_some(30.0));
    if let Some(fps) = fps {
        v.extend(["-framerate".to_string(), format!("{fps}")]);
    }
    if let Some((w, h)) = size {
        v.extend(["-video_size".to_string(), format!("{w}x{h}")]);
    }
    v.extend(["-i".to_string(), source]);
    Ok(v)
}

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

/// Expands directories into their image files (sorted).
pub fn image_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut files: Vec<PathBuf> = fs::read_dir(p)?
                .flatten()
                .map(|e| e.path())
                .filter(|p| is_image(p))
                .collect();
            files.sort();
            out.extend(files);
        } else {
            out.push(p.clone());
        }
    }
    if out.is_empty() {
        bail!("no images found");
    }
    Ok(out)
}

impl Input {
    /// Produces frames into `tx` until the input is exhausted or the
    /// receiver hangs up. Runs on its own thread.
    pub fn produce(self, tx: Sender<Picture>) -> Result<()> {
        match self {
            Input::Images(paths) => {
                for p in paths {
                    let img =
                        image::open(&p).with_context(|| format!("cannot read {}", p.display()))?;
                    let color = img
                        .color()
                        .has_color()
                        .then(|| Color::Rgb(img.to_rgb8().into_raw()));
                    let img = img.into_luma8();
                    let frame = Picture {
                        luma: LumaFrame {
                            width: img.width() as usize,
                            height: img.height() as usize,
                            pixels: img.into_raw(),
                        },
                        color,
                    };
                    if tx.send(frame).is_err() {
                        break;
                    }
                }
                Ok(())
            }
            Input::Video(path) => {
                let is_y4m = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("y4m"));
                if is_y4m {
                    let f = File::open(&path)
                        .with_context(|| format!("cannot open {}", path.display()))?;
                    read_y4m(BufReader::with_capacity(1 << 20, f), &tx, true)
                } else {
                    let input = vec!["-i".to_string(), path.to_string_lossy().into_owned()];
                    let mut child = spawn_ffmpeg(input, &path.display().to_string())?;
                    let stdout = child.stdout.take().unwrap();
                    let result = read_y4m(BufReader::with_capacity(1 << 20, stdout), &tx, true);
                    let _ = child.kill();
                    let status = child.wait()?;
                    if result.is_ok() && !status.success() && !tx.is_empty() {
                        bail!("ffmpeg failed to decode {}", path.display());
                    }
                    result
                }
            }
            Input::Camera { device, size, fps } => {
                let mut child = spawn_ffmpeg(
                    camera_input(&device, size, fps)?,
                    &format!("camera {device:?}"),
                )?;
                let stdout = child.stdout.take().unwrap();
                let result = read_y4m(BufReader::with_capacity(1 << 20, stdout), &tx, false);
                let _ = child.kill();
                let _ = child.wait();
                result.with_context(|| format!("camera {device:?} stopped"))
            }
        }
    }
}

fn spawn_ffmpeg(input: Vec<String>, what: &str) -> Result<Child> {
    Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error"])
        .args(&input)
        .args(["-f", "yuv4mpegpipe", "-pix_fmt", "yuv444p", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("reading {what} needs ffmpeg on PATH (https://ffmpeg.org); .y4m videos work without it")
            } else {
                e.into()
            }
        })
}

fn header_value<'a>(fields: &'a [&'a str], tag: char) -> Option<&'a str> {
    fields.iter().find_map(|f| f.strip_prefix(tag))
}

/// The largest side of a picture read from a video (8K is 7680).
const MAX_SIDE: usize = 16384;

/// Minimal YUV4MPEG2 reader (8 bits per sample).
fn read_y4m<R: BufRead>(mut r: R, tx: &Sender<Picture>, dedupe: bool) -> Result<()> {
    let mut line = String::new();
    r.read_line(&mut line)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.first() != Some(&"YUV4MPEG2") {
        bail!("not a YUV4MPEG2 stream");
    }
    let w: usize = header_value(&fields, 'W').context("Y4M width")?.parse()?;
    let h: usize = header_value(&fields, 'H').context("Y4M height")?.parse()?;
    // (A video from anywhere: its header must not ask for memory at will.)
    if !(1..=MAX_SIDE).contains(&w) || !(1..=MAX_SIDE).contains(&h) {
        bail!("unsupported Y4M picture size {w}×{h} (at most {MAX_SIDE} on each side)");
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    // The size of each of the two chroma planes.
    let chroma = match header_value(&fields, 'C').unwrap_or("420jpeg") {
        "mono" => None,
        // 420p10, 444p12, mono16…
        c if c.starts_with("mono")
            || c.get(3..)
                .and_then(|rest| rest.strip_prefix('p'))
                .is_some_and(|bits| {
                    !bits.is_empty() && bits.bytes().all(|b| b.is_ascii_digit())
                }) =>
        {
            bail!("unsupported Y4M color space C{c} (more than 8 bits per sample)")
        }
        c if c.starts_with("420") => Some((cw, ch)),
        c if c.starts_with("422") => Some((cw, h)),
        c if c.starts_with("444") => Some((w, h)),
        c => bail!("unsupported Y4M color space C{c}"),
    };
    let mut last = None;
    loop {
        let mut frame_line = Vec::new();
        if r.read_until(b'\n', &mut frame_line)? == 0 {
            return Ok(());
        }
        if !frame_line.starts_with(b"FRAME") {
            bail!("corrupt Y4M frame header");
        }
        let mut pixels = vec![0u8; w * h];
        r.read_exact(&mut pixels)?;
        let color = match chroma {
            Some((cw, ch)) => {
                let (mut u, mut v) = (vec![0u8; cw * ch], vec![0u8; cw * ch]);
                r.read_exact(&mut u)?;
                r.read_exact(&mut v)?;
                Some(Color::Yuv { u, v, cw, ch })
            }
            None => None,
        };
        // Recordings repeat frames while the sender holds a code; decoding
        // an identical picture again cannot yield anything new.
        if dedupe {
            let hash = blake3::hash(&pixels);
            if last == Some(hash) {
                continue;
            }
            last = Some(hash);
        }
        let luma = LumaFrame {
            width: w,
            height: h,
            pixels,
        };
        if tx.send(Picture { luma, color }).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_come_apart() {
        // Two pixels, red and cyan, as RGB and as YUV (BT.601, limited range).
        let luma = |pixels: Vec<u8>| LumaFrame {
            width: 2,
            height: 1,
            pixels,
        };
        let rgb = Picture {
            luma: luma(vec![76, 179]),
            color: Some(Color::Rgb(vec![255, 0, 0, 0, 255, 255])),
        };
        let planes = rgb.channels().unwrap();
        assert_eq!(planes[0].pixels, [255, 0]);
        assert_eq!(planes[1].pixels, [0, 255]);
        assert_eq!(planes[2].pixels, [0, 255]);
        let yuv = Picture {
            luma: luma(vec![81, 170]),
            color: Some(Color::Yuv {
                u: vec![90, 166],
                v: vec![240, 16],
                cw: 2,
                ch: 1,
            }),
        };
        let planes = yuv.channels().unwrap();
        let near = |a: u8, b: u8| a.abs_diff(b) <= 3;
        for (plane, want) in planes.iter().zip([[255, 0], [0, 255], [0, 255]]) {
            assert!(near(plane.pixels[0], want[0]) && near(plane.pixels[1], want[1]));
        }
        // No color, nothing to take apart.
        let gray = Picture {
            luma: luma(vec![0, 255]),
            color: None,
        };
        assert!(gray.channels().is_none());
    }

    #[test]
    fn y4m_headers_cannot_ask_for_memory_at_will() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        for header in [
            "YUV4MPEG2 W4294967296 H4294967296 C420jpeg\n",
            "YUV4MPEG2 W100000 H100000 C420jpeg\n",
            "YUV4MPEG2 W0 H10 C420jpeg\n",
        ] {
            let input = format!("{header}FRAME\n");
            assert!(read_y4m(input.as_bytes(), &tx, false).is_err(), "{header}");
        }
        let ok = format!("YUV4MPEG2 W2 H2 Cmono\nFRAME\n{}", "\0".repeat(4));
        read_y4m(ok.as_bytes(), &tx, false).unwrap();
        // A damaged video is an error, whatever is damaged.
        let good = format!("YUV4MPEG2 W4 H2 C420jpeg\nFRAME\n{}", "\x10".repeat(12)).into_bytes();
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..20_000 {
            let mut data = good.clone();
            data.truncate(next() as usize % (data.len() + 1));
            for _ in 0..next() % 3 {
                if !data.is_empty() {
                    let at = next() as usize % data.len();
                    let bytes = b"0123456789WHCFRAME \n\xff";
                    data[at] = bytes[next() as usize % bytes.len()];
                }
            }
            let _ = read_y4m(&data[..], &tx, false);
        }
    }

    #[test]
    fn camera_arguments() {
        if cfg!(windows) {
            assert!(camera_input("", None, None).is_err());
        }
        let args = camera_input("cam", Some((1280, 720)), Some(15.0)).unwrap();
        let at = |flag: &str| args.iter().position(|a| a == flag);
        // What is asked of the camera comes before the camera itself.
        let input = at("-i").unwrap();
        assert_eq!(args[at("-video_size").unwrap() + 1], "1280x720");
        assert_eq!(args[at("-framerate").unwrap() + 1], "15");
        assert!(at("-video_size").unwrap() < input && at("-framerate").unwrap() < input);
        assert!(args[input + 1].contains("cam"));
        // Nothing is asked for unless given (but macOS needs a frame rate).
        let plain = camera_input("cam", None, None).unwrap();
        assert!(!plain.contains(&"-video_size".to_string()));
        assert_eq!(
            plain.contains(&"-framerate".to_string()),
            cfg!(target_os = "macos")
        );
    }
}
