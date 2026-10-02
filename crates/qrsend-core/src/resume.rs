//! Resume codes: compact lists of missing segments. See docs/PROTOCOL.md §8.

const PREFIX: &str = "QSR1-";
const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeCode {
    pub session_id: u32,
    /// Sorted, de-duplicated segment indices (0 = meta).
    pub segments: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResumeError {
    #[error("not a resume code (expected it to start with {PREFIX})")]
    BadPrefix,
    #[error("invalid character in resume code")]
    BadChar,
    #[error("resume code is truncated or malformed")]
    Malformed,
    #[error("resume code checksum mismatch (typo?)")]
    BadChecksum,
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

fn get_leb(data: &[u8], pos: &mut usize) -> Result<u64, ResumeError> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos).ok_or(ResumeError::Malformed)?;
        *pos += 1;
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(ResumeError::Malformed)
}

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut buf, mut bits) = (0u32, 0);
    for &b in data {
        buf = buf << 8 | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[(buf >> bits & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[(buf << (5 - bits) & 31) as usize] as char);
    }
    out
}

fn base32_decode(text: &str) -> Result<Vec<u8>, ResumeError> {
    let mut out = Vec::new();
    let (mut buf, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let c = c.to_ascii_uppercase();
        let v = B32
            .iter()
            .position(|&a| a == c)
            .ok_or(ResumeError::BadChar)? as u32;
        buf = (buf << 5 | v) & 0xFFFF;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

fn ranges(segments: &[u32]) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &s in segments {
        match out.last_mut() {
            Some((start, len)) if *start + *len == s => *len += 1,
            _ => out.push((s, 1)),
        }
    }
    out
}

impl ResumeCode {
    pub fn new(session_id: u32, mut segments: Vec<u32>) -> Self {
        segments.sort_unstable();
        segments.dedup();
        ResumeCode {
            session_id,
            segments,
        }
    }

    pub fn encode(&self) -> String {
        let mut data = self.session_id.to_le_bytes().to_vec();
        let rs = ranges(&self.segments);
        put_leb(&mut data, rs.len() as u64);
        let mut end = 0u64;
        for (start, len) in rs {
            put_leb(&mut data, start as u64 - end);
            put_leb(&mut data, len as u64);
            end = start as u64 + len as u64;
        }
        let check = crc32fast::hash(&data) as u16;
        data.extend_from_slice(&check.to_le_bytes());
        format!("{PREFIX}{}", base32_encode(&data))
    }

    pub fn decode(text: &str) -> Result<Self, ResumeError> {
        let text: String = text.trim().chars().filter(|c| !c.is_whitespace()).collect();
        let body = text
            .strip_prefix(PREFIX)
            .or_else(|| text.strip_prefix(&PREFIX.to_ascii_lowercase()))
            .ok_or(ResumeError::BadPrefix)?;
        let data = base32_decode(body)?;
        if data.len() < 7 {
            return Err(ResumeError::Malformed);
        }
        let (payload, check) = data.split_at(data.len() - 2);
        if (crc32fast::hash(payload) as u16).to_le_bytes() != check {
            return Err(ResumeError::BadChecksum);
        }
        let session_id = u32::from_le_bytes(payload[..4].try_into().unwrap());
        let mut pos = 4;
        let count = get_leb(payload, &mut pos)?;
        let mut segments = Vec::new();
        let mut end = 0u64;
        for _ in 0..count {
            let start = end + get_leb(payload, &mut pos)?;
            let len = get_leb(payload, &mut pos)?;
            end = start.checked_add(len).ok_or(ResumeError::Malformed)?;
            if end > crate::frame::MAX_U24 as u64 + 1 {
                return Err(ResumeError::Malformed);
            }
            segments.extend(start as u32..end as u32);
        }
        if pos != payload.len() {
            return Err(ResumeError::Malformed);
        }
        Ok(ResumeCode {
            session_id,
            segments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for segs in [
            vec![],
            vec![0],
            vec![3, 4, 5, 9, 100, 101, 7000],
            (0..5000).collect(),
        ] {
            let code = ResumeCode::new(0xCAFEBABE, segs);
            let text = code.encode();
            assert!(text.starts_with("QSR1-"));
            assert_eq!(ResumeCode::decode(&text).unwrap(), code);
            assert_eq!(ResumeCode::decode(&text.to_lowercase()).unwrap(), code);
        }
    }

    #[test]
    fn detects_typos() {
        let text = ResumeCode::new(1, vec![1, 2, 3]).encode();
        let mut chars: Vec<char> = text.chars().collect();
        let last = chars.len() - 3;
        chars[last] = if chars[last] == 'A' { 'B' } else { 'A' };
        let typo: String = chars.into_iter().collect();
        assert!(ResumeCode::decode(&typo).is_err());
    }
}
