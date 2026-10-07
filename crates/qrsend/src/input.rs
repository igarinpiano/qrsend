//! Frame sources for the receiver: image files, Y4M video, anything ffmpeg reads.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result, bail};
use crossbeam_channel::Sender;

/// One greyscale picture to scan for QR codes.
pub struct LumaFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub enum Input {
    Images(Vec<PathBuf>),
    Video(PathBuf),
    /// A live camera, captured through ffmpeg (device name/index as ffmpeg expects it).
    Camera(String),
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
fn camera_input(device: &str) -> Result<Vec<String>> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    Ok(if cfg!(target_os = "macos") {
        let mut v = s(&["-f", "avfoundation", "-framerate", "30", "-i"]);
        v.push(format!("{device}:none"));
        v
    } else if cfg!(windows) {
        if device.is_empty() {
            bail!(
                "name the camera: --camera \"<device name>\" (list them with `ffmpeg -list_devices true -f dshow -i dummy`)"
            );
        }
        let mut v = s(&["-f", "dshow", "-i"]);
        v.push(format!("video={device}"));
        v
    } else {
        let mut v = s(&["-f", "v4l2", "-i"]);
        v.push(device.to_string());
        v
    })
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
    pub fn produce(self, tx: Sender<LumaFrame>) -> Result<()> {
        match self {
            Input::Images(paths) => {
                for p in paths {
                    let img = image::open(&p)
                        .with_context(|| format!("cannot read {}", p.display()))?
                        .into_luma8();
                    let frame = LumaFrame {
                        width: img.width() as usize,
                        height: img.height() as usize,
                        pixels: img.into_raw(),
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
            Input::Camera(device) => {
                let mut child =
                    spawn_ffmpeg(camera_input(&device)?, &format!("camera {device:?}"))?;
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
        .args(["-f", "yuv4mpegpipe", "-pix_fmt", "gray", "-strict", "-1", "-"])
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

/// Minimal YUV4MPEG2 reader; only the luma plane is kept.
fn read_y4m<R: BufRead>(mut r: R, tx: &Sender<LumaFrame>, dedupe: bool) -> Result<()> {
    let mut line = String::new();
    r.read_line(&mut line)?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.first() != Some(&"YUV4MPEG2") {
        bail!("not a YUV4MPEG2 stream");
    }
    let w: usize = header_value(&fields, 'W').context("Y4M width")?.parse()?;
    let h: usize = header_value(&fields, 'H').context("Y4M height")?.parse()?;
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let chroma = match header_value(&fields, 'C').unwrap_or("420jpeg") {
        c if c.starts_with("mono") => 0,
        c if c.starts_with("420") => 2 * cw * ch,
        c if c.starts_with("422") => 2 * cw * h,
        c if c.starts_with("444") => 2 * w * h,
        c => bail!("unsupported Y4M colour space C{c}"),
    };
    let mut skip = vec![0u8; chroma];
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
        r.read_exact(&mut skip)?;
        // Recordings repeat frames while the sender holds a code; decoding
        // an identical picture again cannot yield anything new.
        if dedupe {
            let hash = blake3::hash(&pixels);
            if last == Some(hash) {
                continue;
            }
            last = Some(hash);
        }
        if tx
            .send(LumaFrame {
                width: w,
                height: h,
                pixels,
            })
            .is_err()
        {
            return Ok(());
        }
    }
}
