//! Wire format of a single frame (one QR code). See docs/PROTOCOL.md §3.

use crate::{base45, fec};

pub const MAGIC: [u8; 2] = *b"QS";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 22;
pub const CRC_LEN: usize = 4;
/// Bytes of every frame that are not symbol data.
pub const OVERHEAD: usize = HEADER_LEN + CRC_LEN;
pub const FLAG_ENCRYPTED: u8 = 0b0000_0001;
const KNOWN_FLAGS: u8 = FLAG_ENCRYPTED;
pub const META_INDEX: u32 = 0;
pub const MIN_SYMBOL_SIZE: usize = 16;
pub const MAX_SYMBOL_SIZE: usize = u16::MAX as usize;
pub const MIN_SEG_SHIFT: u8 = 12;
pub const MAX_SEG_SHIFT: u8 = 30;
pub const MAX_U24: u32 = 0xFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameHeader {
    pub flags: u8,
    pub session_id: u32,
    pub seg_shift: u8,
    /// Number of body segments (the meta segment is not counted).
    pub seg_count: u32,
    /// 0 is the meta segment, 1..=seg_count are body segments.
    pub seg_index: u32,
    pub seg_len: u32,
    pub esi: u32,
}

impl FrameHeader {
    pub fn is_meta(&self) -> bool {
        self.seg_index == META_INDEX
    }

    pub fn encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub header: FrameHeader,
    pub symbol: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("not a QRSend frame")]
    BadMagic,
    #[error("unsupported frame version {0}")]
    UnsupportedVersion(u8),
    #[error("frame too short")]
    TooShort,
    #[error("frame too long")]
    TooLong,
    #[error("CRC mismatch")]
    BadCrc,
    #[error("unknown flags {0:#04x}")]
    UnknownFlags(u8),
    #[error("invalid frame field: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Base45(#[from] base45::Base45Error),
}

fn put_u24(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes()[..3]);
}

fn get_u24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}

impl Frame {
    pub fn encoded_len(&self) -> usize {
        OVERHEAD + self.symbol.len()
    }

    pub fn encode(&self) -> Vec<u8> {
        let h = &self.header;
        let mut out = Vec::with_capacity(self.encoded_len());
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(h.flags);
        out.extend_from_slice(&h.session_id.to_le_bytes());
        out.push(h.seg_shift);
        put_u24(&mut out, h.seg_count);
        put_u24(&mut out, h.seg_index);
        out.extend_from_slice(&h.seg_len.to_le_bytes());
        put_u24(&mut out, h.esi);
        out.extend_from_slice(&self.symbol);
        let crc = crc32fast::hash(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Parses and validates a frame (§3.1 rules that need no session context).
    pub fn decode(bytes: &[u8]) -> Result<Frame, FrameError> {
        if bytes.len() < 3 {
            return Err(FrameError::TooShort);
        }
        if bytes[..2] != MAGIC {
            return Err(FrameError::BadMagic);
        }
        if bytes[2] != VERSION {
            return Err(FrameError::UnsupportedVersion(bytes[2]));
        }
        if bytes.len() < OVERHEAD + MIN_SYMBOL_SIZE {
            return Err(FrameError::TooShort);
        }
        if bytes.len() > OVERHEAD + MAX_SYMBOL_SIZE {
            return Err(FrameError::TooLong);
        }
        let (body, crc) = bytes.split_at(bytes.len() - CRC_LEN);
        if crc32fast::hash(body).to_le_bytes() != crc {
            return Err(FrameError::BadCrc);
        }
        let flags = body[3];
        if flags & !KNOWN_FLAGS != 0 {
            return Err(FrameError::UnknownFlags(flags));
        }
        let header = FrameHeader {
            flags,
            session_id: u32::from_le_bytes(body[4..8].try_into().unwrap()),
            seg_shift: body[8],
            seg_count: get_u24(&body[9..12]),
            seg_index: get_u24(&body[12..15]),
            seg_len: u32::from_le_bytes(body[15..19].try_into().unwrap()),
            esi: get_u24(&body[19..22]),
        };
        let symbol = body[HEADER_LEN..].to_vec();
        validate(&header, symbol.len())?;
        Ok(Frame { header, symbol })
    }

    pub fn to_qr_text(&self) -> String {
        base45::encode(&self.encode())
    }

    pub fn from_qr_text(text: &str) -> Result<Frame, FrameError> {
        Frame::decode(&base45::decode(text.trim_end())?)
    }
}

pub(crate) fn validate(h: &FrameHeader, symbol_size: usize) -> Result<(), FrameError> {
    if !(MIN_SEG_SHIFT..=MAX_SEG_SHIFT).contains(&h.seg_shift) {
        return Err(FrameError::Invalid("seg_shift"));
    }
    if h.seg_index > h.seg_count {
        return Err(FrameError::Invalid("seg_index"));
    }
    if h.seg_len == 0 {
        return Err(FrameError::Invalid("seg_len"));
    }
    if !h.is_meta() {
        let nominal = 1u64 << h.seg_shift;
        let len = h.seg_len as u64;
        if len > nominal || (h.seg_index < h.seg_count && len != nominal) {
            return Err(FrameError::Invalid("seg_len"));
        }
    }
    if fec::source_symbol_count(h.seg_len, symbol_size) > fec::MAX_SOURCE_SYMBOLS {
        return Err(FrameError::Invalid("seg_len too large for symbol size"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Frame {
        Frame {
            header: FrameHeader {
                flags: FLAG_ENCRYPTED,
                session_id: 0xDEAD_BEEF,
                seg_shift: 20,
                seg_count: 3,
                seg_index: 3,
                seg_len: 1234,
                esi: 0xABCDEF,
            },
            symbol: (0..100u8).collect(),
        }
    }

    #[test]
    fn roundtrip() {
        let f = sample();
        let bytes = f.encode();
        assert_eq!(bytes.len(), f.encoded_len());
        assert_eq!(Frame::decode(&bytes).unwrap(), f);
        assert_eq!(Frame::from_qr_text(&f.to_qr_text()).unwrap(), f);
    }

    #[test]
    fn rejects_corruption() {
        let mut bytes = sample().encode();
        bytes[30] ^= 1;
        assert_eq!(Frame::decode(&bytes), Err(FrameError::BadCrc));
    }

    #[test]
    fn rejects_inconsistent_fields() {
        let mut f = sample();
        f.header.seg_index = 2; // non-last body segment must be full size
        assert!(Frame::decode(&f.encode()).is_err());
        let mut f = sample();
        f.header.seg_index = 4;
        assert!(Frame::decode(&f.encode()).is_err());
        let mut f = sample();
        f.header.flags = 0x80;
        assert_eq!(
            Frame::decode(&f.encode()),
            Err(FrameError::UnknownFlags(0x80))
        );
    }
}
