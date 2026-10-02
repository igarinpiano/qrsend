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

fn fits(chars: usize, p: QrParams) -> bool {
    // Pure alphanumeric content is the worst case for Base45 text; the
    // segment optimiser can only do better (digits pack denser).
    let probe = "A".repeat(chars);
    QrCode::with_version(
        probe.as_bytes(),
        Version::Normal(p.version as i16),
        p.ec.level(),
    )
    .is_ok()
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

pub fn render(text: &str, p: QrParams) -> Result<QrMatrix, QrError> {
    let code = QrCode::with_version(
        text.as_bytes(),
        Version::Normal(p.version as i16),
        p.ec.level(),
    )
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

/// Detects and decodes every QR code in the image; returns their text.
pub fn detect(img: Luma<'_>) -> Vec<String> {
    let (w, px) = (img.width, img.pixels);
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(w, img.height, |x, y| px[y * w + x]);
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|g| g.decode().ok().map(|(_, text)| text))
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
