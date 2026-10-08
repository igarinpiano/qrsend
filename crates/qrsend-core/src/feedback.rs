//! Feedback from the receiver to the sender: what is still missing, and how
//! reception is going. See docs/PROTOCOL.md §11.
//!
//! Feedback is advisory. It carries no authentication, so a sender uses it to
//! send less or to tune itself, never as proof that the data arrived.
//!
//! A receiver only answers a sender that asked for it with a [`SenderNotice`],
//! a small code the sender mixes into its stream.

use crate::base45;

pub const PREFIX: &str = "QSF1-";
pub const NOTICE_PREFIX: &str = "QSC1-";

const NOTICE_WANTS_FEEDBACK: u8 = 0b0000_0001;
const NOTICE_HEARS_SOUND: u8 = 0b0000_0010;
/// Three bits: the codes are shown in red, green, blue (color codes).
const NOTICE_COLORS_SHIFT: u8 = 2;

/// What a sender tells the receiver besides the data: whether it takes
/// feedback, and in which ways. Receivers that do not know notices ignore
/// them (they are not frames).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderNotice {
    pub session_id: u32,
    /// The sender reads feedback codes (off the receiver's screen, or however
    /// else they reach it).
    pub wants_feedback: bool,
    /// The sender listens with a microphone: feedback may come as sound (see
    /// `crate::sound`).
    pub hears_sound: bool,
    /// The colors the codes are stacked in at the moment (bit 0 red, 1
    /// green, 2 blue); 0: black and white, or not told. A receiver cannot
    /// always tell by looking: to a camera that does not keep the colors
    /// apart, color codes look like black-and-white ones.
    pub colors: u8,
}

impl SenderNotice {
    pub fn encode(&self) -> String {
        let mut data = self.session_id.to_le_bytes().to_vec();
        data.push(
            if self.wants_feedback {
                NOTICE_WANTS_FEEDBACK
            } else {
                0
            } | if self.hears_sound {
                NOTICE_HEARS_SOUND
            } else {
                0
            } | ((self.colors & 0b111) << NOTICE_COLORS_SHIFT),
        );
        let check = crc32fast::hash(&data) as u16;
        data.extend_from_slice(&check.to_le_bytes());
        format!("{NOTICE_PREFIX}{}", base45::encode(&data))
    }

    pub fn decode(text: &str) -> Result<Self, FeedbackError> {
        let body = text
            .trim()
            .strip_prefix(NOTICE_PREFIX)
            .ok_or(FeedbackError::BadPrefix)?;
        let data = base45::decode(body).map_err(|_| FeedbackError::Malformed)?;
        if data.len() < 7 {
            return Err(FeedbackError::Malformed);
        }
        let (payload, check) = data.split_at(data.len() - 2);
        if (crc32fast::hash(payload) as u16).to_le_bytes() != check {
            return Err(FeedbackError::Malformed);
        }
        // Bytes after the capability flags belong to a later revision.
        Ok(SenderNotice {
            session_id: u32::from_le_bytes(payload[..4].try_into().unwrap()),
            wants_feedback: payload[4] & NOTICE_WANTS_FEEDBACK != 0,
            hears_sound: payload[4] & NOTICE_HEARS_SOUND != 0,
            colors: (payload[4] >> NOTICE_COLORS_SHIFT) & 0b111,
        })
    }
}

const STATE_COMPLETE: u8 = 0b0000_0001;
const STATE_TRUNCATED: u8 = 0b0000_0010;

/// Ranges of missing segments a code carries at most. A receiver with more
/// holes lists the first ones and sets `truncated`.
pub const MAX_RANGES: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    pub session_id: u32,
    /// Counts up with every feedback message of one receiver run.
    pub seq: u32,
    /// Everything was received, verified and stored.
    pub complete: bool,
    /// `missing` is cut short: every segment after its last range counts as
    /// missing too.
    pub truncated: bool,
    /// Distinct frames of this session read so far.
    pub frames: u64,
    /// Codes still needed (see `receiver::Progress::remaining_symbols`).
    pub remaining_symbols: u64,
    /// The symbol size `remaining_symbols` is counted in (0: not told).
    pub symbol_size: u32,
    /// Missing segments as sorted, non-adjacent `(first, count)` ranges
    /// (0 = meta).
    pub missing: Vec<(u32, u32)>,
    /// What the receiver's camera makes of the codes (all zero: not told).
    pub camera: Camera,
}

/// The colors of a color code worth showing to a camera that sees them as
/// `seen` says ([`Camera::colors`]), out of those `shown` (bit 0 red, 1
/// green, 2 blue): one of each group of colors the camera tells apart.
/// Colors it reads nothing in are left out; of colors that look alike to
/// it, the one cameras resolve best stays (green, then red, then blue).
/// Fewer than two: colors gain nothing with this camera.
pub fn colors_worth_showing(shown: u8, seen: u8) -> u8 {
    const PAIRS: [(u8, u8); 3] = [(0, 1), (0, 2), (1, 2)];
    let has = |mask: u8, color: u8| mask & (1 << color) != 0;
    let mut group = [0u8, 1, 2];
    for (n, (a, b)) in PAIRS.into_iter().enumerate() {
        let both = has(shown & seen, a) && has(shown & seen, b);
        if both && seen & (8 << n) != 0 {
            let (from, to) = (group[b as usize], group[a as usize]);
            for g in &mut group {
                if *g == from {
                    *g = to;
                }
            }
        }
    }
    let mut keep = 0u8;
    let mut groups_kept = 0u8;
    for color in [1u8, 0, 2] {
        let g = group[color as usize];
        if has(shown & seen, color) && groups_kept & (1 << g) == 0 {
            groups_kept |= 1 << g;
            keep |= 1 << color;
        }
    }
    keep
}

/// How the receiver's camera sees the sender's screen, so that the sender
/// can choose speed and density at once instead of feeling its way.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Camera {
    /// Pictures read per second, in tenths.
    pub reads_tenths: u32,
    /// Camera pixels per dot of the codes, in tenths.
    pub dot_tenths: u32,
    /// What the camera makes of the three colors of the picture (color
    /// codes stack one code in each of red, green and blue), found by
    /// reading each color on its own:
    ///
    /// - bits 0–2: codes are read in red, green, blue;
    /// - bits 3–5: red and green, red and blue, green and blue hold the
    ///   same codes.
    ///
    /// 0: not told. Black-and-white codes read the same in every color
    /// (all six bits); so do color codes to a camera that cannot keep the
    /// colors apart, and a sender that is showing colors learns from this
    /// which of them are worth showing.
    pub colors: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeedbackError {
    #[error("not a feedback code")]
    BadPrefix,
    #[error("feedback code is damaged")]
    Malformed,
}

fn put_leb(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn get_leb(data: &[u8], pos: &mut usize) -> Result<u64, FeedbackError> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos).ok_or(FeedbackError::Malformed)?;
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(FeedbackError::Malformed)
}

/// Sorted segment indices as `(first, count)` ranges.
pub fn ranges(segments: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &s in segments {
        match out.last_mut() {
            Some((start, len)) if *start + *len == s => *len += 1,
            _ => out.push((s, 1)),
        }
    }
    out
}

impl Feedback {
    /// Whether the receiver still needs this segment, as far as this message
    /// tells.
    pub fn needs(&self, index: u32) -> bool {
        if self.complete {
            return false;
        }
        let listed = self
            .missing
            .iter()
            .any(|&(start, len)| index >= start && index - start < len);
        let beyond = self
            .missing
            .last()
            .is_none_or(|&(start, len)| index >= start + len);
        listed || (self.truncated && beyond)
    }

    pub fn encode(&self) -> String {
        format!("{PREFIX}{}", base45::encode(&self.to_bytes()))
    }

    pub fn decode(text: &str) -> Result<Self, FeedbackError> {
        let body = text
            .trim()
            .strip_prefix(PREFIX)
            .ok_or(FeedbackError::BadPrefix)?;
        Self::from_bytes(&base45::decode(body).map_err(|_| FeedbackError::Malformed)?)
    }

    /// The message itself, for channels that carry bytes rather than text
    /// (sound): what `encode` writes after the prefix, before Base45.
    pub fn to_bytes(&self) -> Vec<u8> {
        let listed = &self.missing[..self.missing.len().min(MAX_RANGES)];
        let truncated = self.truncated || listed.len() < self.missing.len();
        let mut data = self.session_id.to_le_bytes().to_vec();
        data.push(
            if self.complete { STATE_COMPLETE } else { 0 }
                | if truncated { STATE_TRUNCATED } else { 0 },
        );
        put_leb(&mut data, self.seq as u64);
        put_leb(&mut data, self.frames);
        put_leb(&mut data, self.remaining_symbols);
        put_leb(&mut data, listed.len() as u64);
        let mut end = 0u64;
        for &(start, len) in listed {
            put_leb(&mut data, start as u64 - end);
            put_leb(&mut data, len as u64);
            end = start as u64 + len as u64;
        }
        put_leb(&mut data, self.symbol_size as u64);
        if self.camera != Camera::default() {
            put_leb(&mut data, self.camera.reads_tenths as u64);
            put_leb(&mut data, self.camera.dot_tenths as u64);
            if self.camera.colors != 0 {
                put_leb(&mut data, self.camera.colors as u64);
            }
        }
        let check = crc32fast::hash(&data) as u16;
        data.extend_from_slice(&check.to_le_bytes());
        data
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, FeedbackError> {
        if data.len() < 7 {
            return Err(FeedbackError::Malformed);
        }
        let (payload, check) = data.split_at(data.len() - 2);
        if (crc32fast::hash(payload) as u16).to_le_bytes() != check {
            return Err(FeedbackError::Malformed);
        }
        let session_id = u32::from_le_bytes(payload[..4].try_into().unwrap());
        let state = payload[4];
        let mut pos = 5;
        let seq = get_leb(payload, &mut pos)? as u32;
        let frames = get_leb(payload, &mut pos)?;
        let remaining_symbols = get_leb(payload, &mut pos)?;
        let count = get_leb(payload, &mut pos)?;
        let mut missing = Vec::new();
        let mut end = 0u64;
        for _ in 0..count {
            let start = end + get_leb(payload, &mut pos)?;
            let len = get_leb(payload, &mut pos)?;
            end = start.checked_add(len).ok_or(FeedbackError::Malformed)?;
            if len == 0 || end > crate::frame::MAX_U24 as u64 + 1 {
                return Err(FeedbackError::Malformed);
            }
            missing.push((start as u32, len as u32));
        }
        let symbol_size = if pos < payload.len() {
            get_leb(payload, &mut pos)? as u32
        } else {
            0
        };
        let camera = if pos < payload.len() {
            let reads_tenths = get_leb(payload, &mut pos)? as u32;
            let dot_tenths = get_leb(payload, &mut pos)? as u32;
            let colors = if pos < payload.len() {
                (get_leb(payload, &mut pos)? & 0x3f) as u8
            } else {
                0
            };
            Camera {
                reads_tenths,
                dot_tenths,
                colors,
            }
        } else {
            Camera::default()
        };
        // Anything after that belongs to a later revision of the format.
        Ok(Feedback {
            session_id,
            seq,
            complete: state & STATE_COMPLETE != 0,
            truncated: state & STATE_TRUNCATED != 0,
            frames,
            remaining_symbols,
            symbol_size,
            missing,
            camera,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Feedback {
        Feedback {
            session_id: 0xDEAD_BEEF,
            seq: 300,
            complete: false,
            truncated: false,
            frames: 123_456,
            remaining_symbols: 7_890,
            symbol_size: 181,
            missing: vec![(0, 1), (5, 3), (1000, 24)],
            camera: Camera::default(),
        }
    }

    #[test]
    fn roundtrip() {
        let f = sample();
        let text = f.encode();
        assert!(text.starts_with(PREFIX));
        assert_eq!(Feedback::decode(&text).unwrap(), f);
        // Small enough for a small QR code.
        assert!(text.len() < 55, "{}", text.len());
        assert!(f.needs(0) && f.needs(7) && f.needs(1023));
        assert!(!f.needs(1) && !f.needs(8) && !f.needs(1024));
    }

    #[test]
    fn notice_roundtrip() {
        for (wants_feedback, hears_sound, colors) in
            [(true, false, 0), (false, false, 0b111), (true, true, 0b110)]
        {
            let n = SenderNotice {
                session_id: 0x0102_0304,
                wants_feedback,
                hears_sound,
                colors,
            };
            let text = n.encode();
            assert_eq!(SenderNotice::decode(&text).unwrap(), n);
            // A notice is neither a frame nor feedback.
            assert!(crate::frame::Frame::from_qr_text(&text).is_err());
            assert_eq!(Feedback::decode(&text), Err(FeedbackError::BadPrefix));
        }
        assert_eq!(
            SenderNotice::decode("QSC1-000000000"),
            Err(FeedbackError::Malformed)
        );
    }

    #[test]
    fn complete_needs_nothing() {
        let f = Feedback {
            complete: true,
            missing: vec![],
            ..sample()
        };
        let back = Feedback::decode(&f.encode()).unwrap();
        assert!(back.complete && !back.needs(0) && !back.needs(3));
    }

    #[test]
    fn long_lists_are_cut_conservatively() {
        let f = Feedback {
            missing: (0..100).map(|i| (i * 2, 1)).collect(),
            ..sample()
        };
        let back = Feedback::decode(&f.encode()).unwrap();
        assert!(back.truncated);
        assert_eq!(back.missing.len(), MAX_RANGES);
        // Listed holes are exact; everything after the list counts as missing.
        assert!(back.needs(0) && !back.needs(1) && back.needs(46));
        assert!(back.needs(47) && back.needs(5000));
    }

    #[test]
    fn rejects_damage_and_other_codes() {
        let text = sample().encode();
        assert_eq!(Feedback::decode("QSR1-ABC"), Err(FeedbackError::BadPrefix));
        let mut damaged = text.clone().into_bytes();
        let last = damaged.len() - 1;
        damaged[last] = if damaged[last] == b'0' { b'1' } else { b'0' };
        assert_eq!(
            Feedback::decode(std::str::from_utf8(&damaged).unwrap()),
            Err(FeedbackError::Malformed)
        );
        assert_eq!(
            Feedback::decode(&text[..text.len() - 3]),
            Err(FeedbackError::Malformed)
        );
    }

    #[test]
    fn which_colors_to_show() {
        const ALL: u8 = 0b111;
        // Every color read, none alike: all three.
        assert_eq!(colors_worth_showing(ALL, 0b000_111), 0b111);
        // Red is not read: green and blue.
        assert_eq!(colors_worth_showing(ALL, 0b000_110), 0b110);
        // Red and green look alike: green stays, with blue.
        assert_eq!(colors_worth_showing(ALL, 0b001_111), 0b110);
        // Red and blue look alike: red stays, with green.
        assert_eq!(colors_worth_showing(ALL, 0b010_111), 0b011);
        // Only one is read, or all look alike: nothing to gain.
        assert_eq!(colors_worth_showing(ALL, 0b000_010).count_ones(), 1);
        assert_eq!(colors_worth_showing(ALL, 0b111_111), 0b010);
        // Alike through a third color counts too (red = green, green = blue).
        assert_eq!(colors_worth_showing(ALL, 0b101_111), 0b010);
        // What is not shown any more is not asked about: a blank color
        // shows whatever leaks into it.
        assert_eq!(colors_worth_showing(0b110, 0b001_111), 0b110);
        assert_eq!(colors_worth_showing(0b110, 0b100_111), 0b010);
    }

    #[test]
    fn ignores_fields_added_later() {
        let f = Feedback {
            camera: Camera {
                reads_tenths: 143,
                dot_tenths: 52,
                colors: 0b101_110,
            },
            ..sample()
        };
        assert_eq!(Feedback::decode(&f.encode()).unwrap(), f);
        // Without the number of colors (0.1.3), the rest still reads.
        let plain = Feedback {
            camera: Camera {
                colors: 0,
                ..f.camera
            },
            ..sample()
        };
        assert_eq!(Feedback::decode(&plain.encode()).unwrap(), plain);
        assert!(plain.encode().len() < f.encode().len());
        let data = base45::decode(f.encode().strip_prefix(PREFIX).unwrap()).unwrap();
        let mut longer = data[..data.len() - 2].to_vec();
        longer.extend_from_slice(&[1, 2, 3]);
        let check = crc32fast::hash(&longer) as u16;
        longer.extend_from_slice(&check.to_le_bytes());
        let text = format!("{PREFIX}{}", base45::encode(&longer));
        assert_eq!(Feedback::decode(&text).unwrap(), f);
        // A code without what the camera sees (0.1.2) still reads…
        assert_eq!(Feedback::decode(&sample().encode()).unwrap(), sample());
        // …and so does one without the symbol size (the first draft).
        let data = base45::decode(sample().encode().strip_prefix(PREFIX).unwrap()).unwrap();
        let f = sample();
        let ranges_end = data.len() - 2 - 2;
        let mut shorter = data[..ranges_end].to_vec();
        let check = crc32fast::hash(&shorter) as u16;
        shorter.extend_from_slice(&check.to_le_bytes());
        let old = Feedback::decode(&format!("{PREFIX}{}", base45::encode(&shorter))).unwrap();
        assert_eq!(
            old,
            Feedback {
                symbol_size: 0,
                ..f
            }
        );
    }
}
