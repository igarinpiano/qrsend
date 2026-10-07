//! QR detection for the CLI.
//!
//! - rqrr locates every code in a frame and reports its geometry.
//! - rxing (a ZXing port) is more tolerant of blur, scaling and compression.
//!
//! Dense frames (dozens of codes, small modules) defeat both when run on the
//! whole picture. QRSend lays codes out on a regular lattice, so every decoded
//! code predicts where its neighbors are; each predicted cell is cropped and
//! decoded on its own, which is far more reliable.

use std::collections::{HashSet, VecDeque};

use qrsend_core::frame::Frame;
use qrsend_core::qr::{self, Luma, QUIET};
use rxing::{BarcodeFormat, DecodeHints};

use crate::input::LumaFrame;

type Quad = [(f64, f64); 4];

fn qr_hints() -> DecodeHints {
    DecodeHints {
        PossibleFormats: Some(HashSet::from([BarcodeFormat::QR_CODE])),
        TryHarder: Some(true),
        ..DecodeHints::default()
    }
}

pub fn detect_rxing(f: &LumaFrame) -> Vec<String> {
    rxing::helpers::detect_multiple_in_luma_with_hints(
        f.pixels.clone(),
        f.width as u32,
        f.height as u32,
        &mut qr_hints(),
    )
    .map(|results| results.iter().map(|r| r.getText().to_string()).collect())
    .unwrap_or_default()
}

pub fn detect_rqrr(f: &LumaFrame) -> Vec<String> {
    qr::detect(Luma {
        width: f.width,
        height: f.height,
        pixels: &f.pixels,
    })
}

fn is_frame(text: &str) -> bool {
    Frame::from_qr_text(text).is_ok()
}

/// A crop of the picture and where its top-left corner sits in the picture.
struct Cell {
    frame: LumaFrame,
    origin: (f64, f64),
}

/// Copies the axis-aligned box around `quad`, grown by `margin` pixels.
fn crop(f: &LumaFrame, quad: &Quad, margin: f64) -> Option<Cell> {
    let xs = quad.iter().map(|p| p.0);
    let ys = quad.iter().map(|p| p.1);
    let x0 = (xs.clone().fold(f64::MAX, f64::min) - margin)
        .floor()
        .max(0.0) as usize;
    let y0 = (ys.clone().fold(f64::MAX, f64::min) - margin)
        .floor()
        .max(0.0) as usize;
    let x1 = ((xs.fold(f64::MIN, f64::max) + margin).ceil() as usize).min(f.width);
    let y1 = ((ys.fold(f64::MIN, f64::max) + margin).ceil() as usize).min(f.height);
    if x1 <= x0 + 16 || y1 <= y0 + 16 {
        return None;
    }
    let (w, h) = (x1 - x0, y1 - y0);
    let mut pixels = Vec::with_capacity(w * h);
    for y in y0..y1 {
        pixels.extend_from_slice(&f.pixels[y * f.width + x0..y * f.width + x1]);
    }
    Some(Cell {
        frame: LumaFrame {
            width: w,
            height: h,
            pixels,
        },
        origin: (x0 as f64, y0 as f64),
    })
}

fn upscaled(cell: &LumaFrame, k: usize) -> LumaFrame {
    let (w, h) = (cell.width * k, cell.height * k);
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = &cell.pixels[(y / k) * cell.width..(y / k + 1) * cell.width];
        pixels.extend(row.iter().flat_map(|&p| std::iter::repeat_n(p, k)));
    }
    LumaFrame {
        width: w,
        height: h,
        pixels,
    }
}

/// Text of the QRSend frame in `cell`, and its corners when rqrr located it.
fn decode_once(cell: &LumaFrame) -> Option<(String, Option<Quad>)> {
    let grids = qr::detect_grids(Luma {
        width: cell.width,
        height: cell.height,
        pixels: &cell.pixels,
    });
    let mut located = None;
    for g in grids {
        if let Some(t) = g.text.filter(|t| is_frame(t)) {
            return Some((t, Some(g.corners)));
        }
        located.get_or_insert(g.corners);
    }
    rxing::helpers::detect_in_luma_with_hints(
        cell.pixels.clone(),
        cell.width as u32,
        cell.height as u32,
        Some(BarcodeFormat::QR_CODE),
        &mut qr_hints(),
    )
    .ok()
    .map(|r| r.getText().to_string())
    .filter(|t| is_frame(t))
    .map(|t| (t, located))
}

/// Decodes the code expected at `quad`. Returns its text and its position,
/// refined from what was actually seen so that errors do not accumulate as
/// the lattice is walked. Tiny modules are a common cause of failure, so a
/// failed crop is retried enlarged.
fn decode_cell(f: &LumaFrame, quad: &Quad, modules: usize) -> Option<(String, Quad)> {
    let module = (quad[1].0 - quad[0].0).hypot(quad[1].1 - quad[0].1) / modules as f64;
    let cell = crop(f, quad, module * 3.5)?;
    let in_picture =
        |q: Quad, k: f64| q.map(|(x, y)| (x / k + cell.origin.0, y / k + cell.origin.1));
    let (text, seen, k) = decode_once(&cell.frame)
        .map(|(t, q)| (t, q, 1.0))
        .or_else(|| {
            [2usize, 3]
                .into_iter()
                .find_map(|k| decode_once(&upscaled(&cell.frame, k)).map(|(t, q)| (t, q, k as f64)))
        })?;
    // Trust a refined position only if it agrees with the prediction.
    let refined = seen.map(|q| in_picture(q, k)).filter(|r| {
        r.iter()
            .zip(quad)
            .all(|(a, b)| (a.0 - b.0).hypot(a.1 - b.1) < module * 3.0)
    });
    Some((text, refined.unwrap_or(*quad)))
}

fn shifted(q: &Quad, d: (f64, f64)) -> Quad {
    q.map(|(x, y)| (x + d.0, y + d.1))
}

fn inside(f: &LumaFrame, q: &Quad) -> bool {
    q.iter().all(|&(x, y)| {
        x >= -2.0 && y >= -2.0 && x <= f.width as f64 + 2.0 && y <= f.height as f64 + 2.0
    })
}

/// Cell positions found in an earlier frame of the same video. A played-back
/// stream keeps its layout, so later frames can skip the full-picture search.
#[derive(Default, Clone)]
pub struct Lattice {
    size: (usize, usize),
    cells: Vec<(Quad, usize)>,
}

fn try_lattice(f: &LumaFrame, lattice: &Lattice) -> Option<Vec<String>> {
    if lattice.size != (f.width, f.height) || lattice.cells.len() < 2 {
        return None;
    }
    let mut texts = Vec::new();
    for (i, (q, m)) in lattice.cells.iter().enumerate() {
        if let Some((text, _)) = decode_cell(f, q, *m)
            && !texts.contains(&text)
        {
            texts.push(text);
        }
        // The layout changed (or this is live footage): search from scratch.
        if i == 3 && texts.is_empty() {
            return None;
        }
    }
    (texts.len() * 2 >= lattice.cells.len()).then_some(texts)
}

/// [`detect`] with a lattice remembered between frames.
pub fn detect_cached(f: &LumaFrame, cache: &std::sync::Mutex<Lattice>) -> Vec<String> {
    let known = cache.lock().unwrap().clone();
    if let Some(texts) = try_lattice(f, &known) {
        return texts;
    }
    let (texts, cells) = detect_with_cells(f);
    if cells.len() >= 2 {
        *cache.lock().unwrap() = Lattice {
            size: (f.width, f.height),
            cells,
        };
    }
    texts
}

/// Finds QRSend frames in a picture; other QR codes are returned too when the
/// picture holds no QRSend frame at all (device IDs).
pub fn detect(f: &LumaFrame) -> Vec<String> {
    detect_with_cells(f).0
}

/// One QRSend code and its geometry, found without searching the whole
/// picture for every code (which gets very slow with hundreds of codes):
/// rxing picks any one code, then rqrr measures it in a crop around it.
fn quick_seed(f: &LumaFrame) -> Option<(String, Quad, usize)> {
    let hit = rxing::helpers::detect_in_luma_with_hints(
        f.pixels.clone(),
        f.width as u32,
        f.height as u32,
        Some(BarcodeFormat::QR_CODE),
        &mut qr_hints(),
    )
    .ok()?;
    let text = hit.getText().to_string();
    let pts = hit.getPoints();
    if !is_frame(&text) || pts.len() < 3 {
        return None;
    }
    // Finder-pattern centers; the code extends a little beyond them.
    let span = (pts[1].x - pts[2].x).hypot(pts[1].y - pts[2].y) as f64;
    let around: Quad = [0, 1, 2, 0].map(|i| (pts[i].x as f64, pts[i].y as f64));
    let cell = crop(f, &around, span * 0.6)?;
    [1usize, 2].into_iter().find_map(|k| {
        let img = if k == 1 {
            None
        } else {
            Some(upscaled(&cell.frame, k))
        };
        let img = img.as_ref().unwrap_or(&cell.frame);
        qr::detect_grids(Luma {
            width: img.width,
            height: img.height,
            pixels: &img.pixels,
        })
        .into_iter()
        .find(|g| g.text.as_deref() == Some(text.as_str()))
        .map(|g| {
            let k = k as f64;
            (
                text.clone(),
                g.corners
                    .map(|(x, y)| (x / k + cell.origin.0, y / k + cell.origin.1)),
                g.modules,
            )
        })
    })
}

fn detect_with_cells(f: &LumaFrame) -> (Vec<String>, Vec<(Quad, usize)>) {
    let mut texts: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(Quad, usize)> = VecDeque::new();
    let mut pending: Vec<(Quad, usize)> = Vec::new();
    if let Some((text, quad, modules)) = quick_seed(f) {
        seen.insert(text.clone());
        texts.push(text);
        queue.push_back((quad, modules));
    } else {
        let grids = qr::detect_grids(Luma {
            width: f.width,
            height: f.height,
            pixels: &f.pixels,
        });
        for g in &grids {
            match &g.text {
                Some(t) if is_frame(t) => {
                    if seen.insert(t.clone()) {
                        texts.push(t.clone());
                    }
                    queue.push_back((g.corners, g.modules));
                }
                // Located but unreadable in the full picture: retry as a crop.
                _ => pending.push((g.corners, g.modules)),
            }
        }
        if queue.is_empty() {
            // Blurry or scaled input, or no QRSend frame at all: let rxing
            // look at the whole picture.
            let mut all: Vec<String> = grids.into_iter().filter_map(|g| g.text).collect();
            for t in detect_rxing(f) {
                if !all.contains(&t) {
                    all.push(t);
                }
            }
            return (all, Vec::new());
        }
    }

    // Lattice cells already handled, keyed by their center on a coarse raster.
    let key = |q: &Quad, m: usize| {
        let side = ((q[1].0 - q[0].0).hypot(q[1].1 - q[0].1)).max(1.0);
        let cell = side * (m + QUIET) as f64 / m as f64 / 2.0;
        let (cx, cy) = (
            q.iter().map(|p| p.0).sum::<f64>() / 4.0,
            q.iter().map(|p| p.1).sum::<f64>() / 4.0,
        );
        ((cx / cell).round() as i64, (cy / cell).round() as i64)
    };
    let mut visited: HashSet<(i64, i64)> = queue.iter().map(|(q, m)| key(q, *m)).collect();
    let mut cells: Vec<(Quad, usize)> = queue.iter().cloned().collect();
    while let Some((q, m)) = queue.pop_front() {
        // One lattice step along each edge of this code.
        let k = (m + QUIET) as f64 / m as f64;
        let right = ((q[1].0 - q[0].0) * k, (q[1].1 - q[0].1) * k);
        let down = ((q[3].0 - q[0].0) * k, (q[3].1 - q[0].1) * k);
        for d in [right, (-right.0, -right.1), down, (-down.0, -down.1)] {
            let n = shifted(&q, d);
            if !inside(f, &n) || !visited.insert(key(&n, m)) {
                continue;
            }
            if let Some((text, at)) = decode_cell(f, &n, m) {
                if seen.insert(text.clone()) {
                    texts.push(text);
                }
                cells.push((at, m));
                queue.push_back((at, m));
            }
        }
    }
    // Codes rqrr located but could not read, outside the lattice found so far.
    // Their geometry is less trustworthy, so they do not extend the lattice.
    for (q, m) in pending {
        if inside(f, &q)
            && !visited.contains(&key(&q, m))
            && let Some((text, _)) = decode_cell(f, &q, m)
            && seen.insert(text.clone())
        {
            texts.push(text);
        }
    }
    (texts, cells)
}
