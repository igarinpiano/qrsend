//! Base45 encoding (RFC 9285).
//!
//! The Base45 alphabet is exactly the QR alphanumeric character set, so frames
//! encoded with it are stored in alphanumeric mode (~97% of byte-mode density)
//! and survive decoders that only return text.

const ALPHABET: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// Length of the Base45 encoding of `n` bytes.
pub const fn encoded_len(n: usize) -> usize {
    n / 2 * 3 + (n % 2) * 2
}

/// Largest number of bytes whose Base45 encoding fits in `chars` characters.
pub const fn max_bytes_for_chars(chars: usize) -> usize {
    chars / 3 * 2 + if chars % 3 == 2 { 1 } else { 0 }
}

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(encoded_len(data.len()));
    let mut chunks = data.chunks_exact(2);
    for pair in &mut chunks {
        let n = (pair[0] as u32) << 8 | pair[1] as u32;
        out.push(ALPHABET[(n % 45) as usize] as char);
        out.push(ALPHABET[(n / 45 % 45) as usize] as char);
        out.push(ALPHABET[(n / 2025) as usize] as char);
    }
    if let [b] = chunks.remainder() {
        let n = *b as u32;
        out.push(ALPHABET[(n % 45) as usize] as char);
        out.push(ALPHABET[(n / 45) as usize] as char);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Base45Error {
    #[error("invalid Base45 character {0:?}")]
    InvalidChar(char),
    #[error("invalid Base45 length")]
    InvalidLength,
    #[error("Base45 group out of range")]
    OutOfRange,
}

fn value(c: u8) -> Result<u32, Base45Error> {
    ALPHABET
        .iter()
        .position(|&a| a == c)
        .map(|p| p as u32)
        .ok_or(Base45Error::InvalidChar(c as char))
}

pub fn decode(text: &str) -> Result<Vec<u8>, Base45Error> {
    let bytes = text.as_bytes();
    if bytes.len() % 3 == 1 {
        return Err(Base45Error::InvalidLength);
    }
    let mut out = Vec::with_capacity(max_bytes_for_chars(bytes.len()));
    let mut chunks = bytes.chunks_exact(3);
    for g in &mut chunks {
        let n = value(g[0])? + value(g[1])? * 45 + value(g[2])? * 2025;
        if n > 0xFFFF {
            return Err(Base45Error::OutOfRange);
        }
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    if let [a, b] = chunks.remainder() {
        let n = value(*a)? + value(*b)? * 45;
        if n > 0xFF {
            return Err(Base45Error::OutOfRange);
        }
        out.push(n as u8);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc9285_vectors() {
        assert_eq!(encode(b"AB"), "BB8");
        assert_eq!(encode(b"Hello!!"), "%69 VD92EX0");
        assert_eq!(encode(b"base-45"), "UJCLQE7W581");
        assert_eq!(decode("QED8WEX0").unwrap(), b"ietf!");
        assert!(decode("GGW").is_err());
    }

    #[test]
    fn roundtrip_all_lengths() {
        for len in 0..64 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + 11) as u8).collect();
            let enc = encode(&data);
            assert_eq!(enc.len(), encoded_len(len));
            assert_eq!(max_bytes_for_chars(enc.len()), len);
            assert_eq!(decode(&enc).unwrap(), data);
        }
    }
}
