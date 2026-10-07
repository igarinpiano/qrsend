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

/// What a sender tells the receiver besides the data: so far only whether it
/// can read feedback codes. Receivers that do not know notices ignore them
/// (they are not frames).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderNotice {
    pub session_id: u32,
    pub wants_feedback: bool,
}

impl SenderNotice {
    pub fn encode(&self) -> String {
        let mut data = self.session_id.to_le_bytes().to_vec();
        data.push(if self.wants_feedback {
            NOTICE_WANTS_FEEDBACK
        } else {
            0
        });
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
    /// Missing segments as sorted, non-adjacent `(first, count)` ranges
    /// (0 = meta).
    pub missing: Vec<(u32, u32)>,
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
        let check = crc32fast::hash(&data) as u16;
        data.extend_from_slice(&check.to_le_bytes());
        format!("{PREFIX}{}", base45::encode(&data))
    }

    pub fn decode(text: &str) -> Result<Self, FeedbackError> {
        let body = text
            .trim()
            .strip_prefix(PREFIX)
            .ok_or(FeedbackError::BadPrefix)?;
        let data = base45::decode(body).map_err(|_| FeedbackError::Malformed)?;
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
        // Anything after the ranges belongs to a later revision of the format.
        Ok(Feedback {
            session_id,
            seq,
            complete: state & STATE_COMPLETE != 0,
            truncated: state & STATE_TRUNCATED != 0,
            frames,
            remaining_symbols,
            missing,
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
            missing: vec![(0, 1), (5, 3), (1000, 24)],
        }
    }

    #[test]
    fn roundtrip() {
        let f = sample();
        let text = f.encode();
        assert!(text.starts_with(PREFIX));
        assert_eq!(Feedback::decode(&text).unwrap(), f);
        // Small enough for a small QR code.
        assert!(text.len() < 50, "{}", text.len());
        assert!(f.needs(0) && f.needs(7) && f.needs(1023));
        assert!(!f.needs(1) && !f.needs(8) && !f.needs(1024));
    }

    #[test]
    fn notice_roundtrip() {
        for wants_feedback in [true, false] {
            let n = SenderNotice {
                session_id: 0x0102_0304,
                wants_feedback,
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
    fn ignores_fields_added_later() {
        let f = sample();
        let data = base45::decode(f.encode().strip_prefix(PREFIX).unwrap()).unwrap();
        let mut longer = data[..data.len() - 2].to_vec();
        longer.extend_from_slice(&[1, 2, 3]);
        let check = crc32fast::hash(&longer) as u16;
        longer.extend_from_slice(&check.to_le_bytes());
        let text = format!("{PREFIX}{}", base45::encode(&longer));
        assert_eq!(Feedback::decode(&text).unwrap(), f);
    }
}
