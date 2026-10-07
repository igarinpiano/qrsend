//! QR rendering and detection helpers (pure Rust; usable from WASM).

use qrcode::{EcLevel, QrCode, Version};

use crate::{base45, frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ec {
    L,
    M,
    Q,
    H,
}

impl Ec {
    fn level(self) -> EcLevel {
        match self {
            Ec::L => EcLevel::L,
            Ec::M => EcLevel::M,
            Ec::Q => EcLevel::Q,
            Ec::H => EcLevel::H,
        }
    }
}

impl std::str::FromStr for Ec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_uppercase().as_str() {
            "L" => Ok(Ec::L),
            "M" => Ok(Ec::M),
            "Q" => Ok(Ec::Q),
            "H" => Ok(Ec::H),
            _ => Err(format!(
                "unknown error correction level {s:?} (use L, M, Q or H)"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QrParams {
    pub version: u8,
    pub ec: Ec,
}

/// Density presets (docs/PROTOCOL.md §2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Low,
    Normal,
    High,
    Max,
}

impl Density {
    pub fn params(self) -> QrParams {
        match self {
            Density::Low => QrParams {
                version: 15,
                ec: Ec::M,
            },
            Density::Normal => QrParams {
                version: 25,
                ec: Ec::L,
            },
            Density::High => QrParams {
                version: 32,
                ec: Ec::L,
            },
            Density::Max => QrParams {
                version: 40,
                ec: Ec::L,
            },
        }
    }
}

impl std::str::FromStr for Density {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "low" => Ok(Density::Low),
            "normal" => Ok(Density::Normal),
            "high" => Ok(Density::High),
            "max" => Ok(Density::Max),
            _ => Err(format!(
                "unknown density {s:?} (use low, normal, high or max)"
            )),
        }
    }
}

/// Encodes `text` as one alphanumeric segment. Base45 output is always valid
/// alphanumeric data, and a single segment makes capacity exact (the generic
/// segment optimiser can pick mixes that overflow a fixed version).
#[cfg(test)]
fn encode_alphanumeric(text: &[u8], p: QrParams) -> qrcode::types::QrResult<QrCode> {
    let mut bits = qrcode::bits::Bits::new(Version::Normal(p.version as i16));
    bits.push_alphanumeric_data(text)?;
    bits.push_terminator(p.ec.level())?;
    QrCode::with_bits(bits, p.ec.level())
}

fn fits(chars: usize, p: QrParams) -> bool {
    let mut bits = qrcode::bits::Bits::new(Version::Normal(p.version as i16));
    bits.push_alphanumeric_data("A".repeat(chars).as_bytes())
        .is_ok()
        && bits.push_terminator(p.ec.level()).is_ok()
}

impl QrParams {
    /// Number of Base45 characters that always fit.
    pub fn capacity_chars(self) -> usize {
        assert!(
            (1..=40).contains(&self.version),
            "QR version must be 1..=40"
        );
        let (mut lo, mut hi) = (0usize, 4296usize);
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if fits(mid, self) {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        lo
    }

    /// Largest frame (bytes) that fits.
    pub fn frame_capacity(self) -> usize {
        base45::max_bytes_for_chars(self.capacity_chars())
    }

    /// Symbol size `T` for frames of this QR configuration.
    pub fn symbol_size(self) -> usize {
        self.frame_capacity().saturating_sub(frame::OVERHEAD)
    }

    /// Modules per side (without quiet zone).
    pub fn modules(self) -> usize {
        17 + 4 * self.version as usize
    }
}

/// Gap between neighbouring codes and around the grid, in modules. Adjacent
/// codes share it, so every code still has the 4-module quiet zone.
pub const QUIET: usize = 4;

/// Picks a QR size for a transfer: small transfers get small, easy-to-scan
/// codes instead of mostly-padding large ones; everything else gets `Normal`.
pub fn auto_params(meta_len: u32, body_len: u64) -> QrParams {
    const SMALL: [QrParams; 3] = [
        QrParams {
            version: 10,
            ec: Ec::M,
        },
        QrParams {
            version: 15,
            ec: Ec::M,
        },
        QrParams {
            version: 20,
            ec: Ec::M,
        },
    ];
    for p in SMALL {
        let t = p.symbol_size() as u64;
        if (meta_len as u64).div_ceil(t) + body_len.div_ceil(t) <= 4 {
            return p;
        }
    }
    Density::Normal.params()
}

/// Modules spanned by `n` codes of `modules` width laid side by side.
pub fn grid_extent(modules: usize, n: usize) -> usize {
    n * (modules + QUIET) + QUIET
}

/// How many codes fit along `px` pixels at `scale` pixels per module.
pub fn grid_fit(modules: usize, px: usize, scale: usize) -> usize {
    (px / scale.max(1)).saturating_sub(QUIET) / (modules + QUIET)
}

/// A grid of codes filling a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiling {
    pub params: QrParams,
    pub cols: usize,
    pub rows: usize,
}

impl Tiling {
    /// Payload bytes carried by one frame.
    pub fn capacity(&self) -> usize {
        self.cols * self.rows * self.params.symbol_size()
    }
}

/// The QR version (ECC L) and grid that carry the most data in a
/// `width × height` pixel frame at `scale` pixels per module.
pub fn best_tiling(width: usize, height: usize, scale: usize) -> Option<Tiling> {
    (1..=40u8)
        .filter_map(|version| {
            let params = QrParams { version, ec: Ec::L };
            if params.symbol_size() < frame::MIN_SYMBOL_SIZE {
                return None;
            }
            let (cols, rows) = (
                grid_fit(params.modules(), width, scale),
                grid_fit(params.modules(), height, scale),
            );
            (cols > 0 && rows > 0).then_some(Tiling { params, cols, rows })
        })
        .max_by_key(|t| (t.capacity(), std::cmp::Reverse(t.params.version)))
}

/// A rendered QR symbol: `width × width` modules, `true` = dark.
#[derive(Debug, Clone)]
pub struct QrMatrix {
    pub width: usize,
    pub modules: Vec<bool>,
}

impl QrMatrix {
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.width + x]
    }
}

#[derive(Debug, thiserror::Error)]
#[error("QR encoding failed: {0}")]
pub struct QrError(String);

/// Renders a frame's Base45 text as one alphanumeric segment of exactly
/// version `p.version`.
///
/// The mask is fixed instead of being chosen by scoring all eight candidates:
/// that scoring dominates the cost of generating a code, any mask decodes the
/// same, and QRSend payloads are compressed or encrypted (already uniform), so
/// the "best" mask buys nothing. This makes dense grids several times faster.
pub fn render(text: &str, p: QrParams) -> Result<QrMatrix, QrError> {
    use fast_qr::{ECL, Mask, Mode, Version::*};
    const VERSIONS: [fast_qr::Version; 40] = [
        V01, V02, V03, V04, V05, V06, V07, V08, V09, V10, V11, V12, V13, V14, V15, V16, V17, V18,
        V19, V20, V21, V22, V23, V24, V25, V26, V27, V28, V29, V30, V31, V32, V33, V34, V35, V36,
        V37, V38, V39, V40,
    ];
    if !(1..=40).contains(&p.version) {
        return Err(QrError("QR version must be 1..=40".into()));
    }
    let ecl = match p.ec {
        Ec::L => ECL::L,
        Ec::M => ECL::M,
        Ec::Q => ECL::Q,
        Ec::H => ECL::H,
    };
    let code = fast_qr::QRBuilder::new(text.as_bytes())
        .mode(Mode::Alphanumeric)
        .version(VERSIONS[p.version as usize - 1])
        .ecl(ecl)
        .mask(Mask::Checkerboard)
        .build()
        .map_err(|_| QrError("data too long".into()))?;
    let width = code.size;
    if width != p.modules() {
        return Err(QrError("data too long".into()));
    }
    let modules = code.data[..width * width]
        .iter()
        .map(|m| m.value())
        .collect();
    Ok(QrMatrix { width, modules })
}

/// The same code through the `qrcode` crate (reference for tests).
#[cfg(test)]
fn render_reference(text: &str, p: QrParams) -> Result<QrMatrix, QrError> {
    let code = encode_alphanumeric(text.as_bytes(), p).map_err(|e| QrError(e.to_string()))?;
    let width = code.width();
    let modules = code
        .into_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix { width, modules })
}

/// Renders arbitrary text (any mode, smallest version, ECC M) — for device IDs.
pub fn render_text(text: &str) -> Result<QrMatrix, QrError> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|e| QrError(e.to_string()))?;
    let width = code.width();
    let modules = code
        .into_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix { width, modules })
}

/// 8-bit greyscale image view used for detection.
pub struct Luma<'a> {
    pub width: usize,
    pub height: usize,
    pub pixels: &'a [u8],
}

/// A QR code located in an image.
#[derive(Debug, Clone)]
pub struct Found {
    /// Corners in pixels: top-left, top-right, bottom-right, bottom-left.
    pub corners: [(f64, f64); 4],
    /// Modules per side.
    pub modules: usize,
    /// The decoded text, when decoding succeeded.
    pub text: Option<String>,
}

/// Locates every QR code in the image, decoded or not.
pub fn detect_grids(img: Luma<'_>) -> Vec<Found> {
    use rqrr::BitGrid;
    let (w, px) = (img.width, img.pixels);
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(w, img.height, |x, y| px[y * w + x]);
    prepared
        .detect_grids()
        .into_iter()
        .map(|g| Found {
            corners: g.bounds.map(|p| (p.x as f64, p.y as f64)),
            modules: g.grid.size(),
            text: g.decode().ok().map(|(_, text)| text),
        })
        .collect()
}

/// Detects and decodes every QR code in the image; returns their text.
pub fn detect(img: Luma<'_>) -> Vec<String> {
    detect_grids(img)
        .into_iter()
        .filter_map(|f| f.text)
        .collect()
}

/// Rasterises a matrix with a quiet zone into a greyscale buffer (0 = dark).
pub fn rasterize(m: &QrMatrix, scale: usize, quiet: usize) -> (usize, Vec<u8>) {
    let side = (m.width + 2 * quiet) * scale;
    let mut px = vec![255u8; side * side];
    for y in 0..m.width {
        for x in 0..m.width {
            if m.dark(x, y) {
                let (ox, oy) = ((x + quiet) * scale, (y + quiet) * scale);
                for dy in 0..scale {
                    let row = (oy + dy) * side;
                    px[row + ox..row + ox + scale].fill(0);
                }
            }
        }
    }
    (side, px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacities_are_sane() {
        let normal = Density::Normal.params();
        // v25-L alphanumeric capacity is 1853 characters.
        assert_eq!(normal.capacity_chars(), 1853);
        assert_eq!(normal.frame_capacity(), 1235);
        assert_eq!(Density::Max.params().capacity_chars(), 4296);
    }

    #[test]
    fn full_capacity_always_renders() {
        for p in [
            Density::Low.params(),
            Density::Normal.params(),
            QrParams {
                version: 7,
                ec: Ec::H,
            },
        ] {
            for seed in 0..20u32 {
                let bytes: Vec<u8> = (0..p.frame_capacity())
                    .map(|i| (i as u32).wrapping_mul(2654435761) as u8 ^ seed as u8)
                    .collect();
                render(&base45::encode(&bytes), p).unwrap();
            }
        }
    }

    #[test]
    fn auto_density_shrinks_small_transfers() {
        assert_eq!(auto_params(150, 20).version, 10);
        assert_eq!(auto_params(600, 300).version, 15);
        assert_eq!(auto_params(600, 5_000_000), Density::Normal.params());
    }

    #[test]
    fn tiling_fills_the_frame() {
        let t = best_tiling(1920, 1080, 2).unwrap();
        let m = t.params.modules();
        assert!(grid_extent(m, t.cols) * 2 <= 1920 && grid_extent(m, t.rows) * 2 <= 1080);
        // Better than one row of the largest code.
        let v40 = QrParams {
            version: 40,
            ec: Ec::L,
        };
        assert!(
            t.capacity() >= grid_fit(177, 1920, 2) * grid_fit(177, 1080, 2) * v40.symbol_size()
        );
        assert!(best_tiling(40, 40, 2).is_none());
    }

    #[test]
    fn every_version_decodes_and_overflow_is_rejected() {
        for version in [3u8, 10, 15, 25, 32, 40] {
            for ec in [Ec::L, Ec::M] {
                let p = QrParams { version, ec };
                let bytes: Vec<u8> = (0..p.frame_capacity())
                    .map(|i| (i * 31 + 7) as u8)
                    .collect();
                let text = base45::encode(&bytes);
                let m = render(&text, p).unwrap();
                assert_eq!(m.width, p.modules());
                let (side, px) = rasterize(&m, 3, 4);
                assert_eq!(
                    detect(Luma {
                        width: side,
                        height: side,
                        pixels: &px
                    }),
                    vec![text.clone()],
                    "v{version}"
                );
                // One group more than the capacity must not silently grow the code.
                assert!(render(&format!("{text}AAAA"), p).is_err());
                assert!(render_reference(&text, p).is_ok());
            }
        }
    }

    #[test]
    #[ignore = "timing comparison; run with --ignored --nocapture"]
    fn render_speed() {
        let p = Density::Normal.params();
        let bytes: Vec<u8> = (0..p.frame_capacity())
            .map(|i| (i * 31 + 7) as u8)
            .collect();
        let text = base45::encode(&bytes);
        let t = std::time::Instant::now();
        for _ in 0..50 {
            render(&text, p).unwrap();
        }
        let fast = t.elapsed() / 50;
        let t = std::time::Instant::now();
        for _ in 0..50 {
            render_reference(&text, p).unwrap();
        }
        println!(
            "fast_qr fixed mask {fast:?} per code · qrcode {:?} per code",
            t.elapsed() / 50
        );
    }

    #[test]
    fn render_and_detect_roundtrip() {
        let p = QrParams {
            version: 10,
            ec: Ec::M,
        };
        let bytes: Vec<u8> = (0..p.frame_capacity()).map(|i| (i * 7) as u8).collect();
        let text = base45::encode(&bytes);
        let m = render(&text, p).unwrap();
        let (side, px) = rasterize(&m, 4, 4);
        let found = detect(Luma {
            width: side,
            height: side,
            pixels: &px,
        });
        assert_eq!(found, vec![text]);
    }
}
