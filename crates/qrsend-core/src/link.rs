//! Link codes: small messages two devices exchange to set up another channel
//! between them (so far a direct connection over the local network), carried
//! like any other code. See docs/PROTOCOL.md §12.
//!
//! The content is opaque here; this module only frames it: which transfer it
//! belongs to, what it is, and how the parts of a long message fit together.

use crate::base45;

pub const PREFIX: &str = "QSL1-";

/// The sender offers a direct connection.
pub const KIND_OFFER: u8 = 1;
/// The receiver accepts it.
pub const KIND_ANSWER: u8 = 2;

const HEADER_LEN: usize = 8;
const CHECK_LEN: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkPart {
    pub session_id: u32,
    pub kind: u8,
    /// Tells the parts of one message from those of another of the same kind.
    pub id: u8,
    /// 0-based index of this part.
    pub part: u8,
    pub parts: u8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("not a link code")]
    BadPrefix,
    #[error("link code is damaged")]
    Malformed,
    #[error("message does not fit into 255 codes of this size")]
    TooLong,
}

impl LinkPart {
    pub fn encode(&self) -> String {
        let mut data = self.session_id.to_le_bytes().to_vec();
        data.extend_from_slice(&[self.kind, self.id, self.part, self.parts]);
        data.extend_from_slice(&self.payload);
        let check = crc32fast::hash(&data) as u16;
        data.extend_from_slice(&check.to_le_bytes());
        format!("{PREFIX}{}", base45::encode(&data))
    }

    pub fn decode(text: &str) -> Result<Self, LinkError> {
        let body = text
            .trim()
            .strip_prefix(PREFIX)
            .ok_or(LinkError::BadPrefix)?;
        let data = base45::decode(body).map_err(|_| LinkError::Malformed)?;
        if data.len() < HEADER_LEN + CHECK_LEN {
            return Err(LinkError::Malformed);
        }
        let (payload, check) = data.split_at(data.len() - CHECK_LEN);
        if (crc32fast::hash(payload) as u16).to_le_bytes() != check {
            return Err(LinkError::Malformed);
        }
        let part = LinkPart {
            session_id: u32::from_le_bytes(payload[..4].try_into().unwrap()),
            kind: payload[4],
            id: payload[5],
            part: payload[6],
            parts: payload[7],
            payload: payload[HEADER_LEN..].to_vec(),
        };
        if part.parts == 0 || part.part >= part.parts {
            return Err(LinkError::Malformed);
        }
        Ok(part)
    }
}

/// Splits a message into codes of at most `max_chars` characters each.
pub fn split(
    session_id: u32,
    kind: u8,
    id: u8,
    payload: &[u8],
    max_chars: usize,
) -> Result<Vec<String>, LinkError> {
    let room = base45::max_bytes_for_chars(max_chars.saturating_sub(PREFIX.len()))
        .saturating_sub(HEADER_LEN + CHECK_LEN);
    if room == 0 {
        return Err(LinkError::TooLong);
    }
    let chunks: Vec<&[u8]> = if payload.is_empty() {
        vec![&[]]
    } else {
        payload.chunks(room).collect()
    };
    let parts = u8::try_from(chunks.len()).map_err(|_| LinkError::TooLong)?;
    Ok(chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            LinkPart {
                session_id,
                kind,
                id,
                part: i as u8,
                parts,
                payload: chunk.to_vec(),
            }
            .encode()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_reassemble() {
        let payload: Vec<u8> = (0..500u32).map(|i| (i * 7) as u8).collect();
        let codes = split(0xABCD_0123, KIND_OFFER, 9, &payload, 120).unwrap();
        assert!(codes.len() > 1);
        let mut back = Vec::new();
        for (i, code) in codes.iter().enumerate() {
            assert!(code.len() <= 120, "{}", code.len());
            let p = LinkPart::decode(code).unwrap();
            assert_eq!(
                (
                    p.session_id,
                    p.kind,
                    p.id,
                    p.part as usize,
                    p.parts as usize
                ),
                (0xABCD_0123, KIND_OFFER, 9, i, codes.len())
            );
            back.extend(p.payload);
        }
        assert_eq!(back, payload);
        // A link code is not a frame.
        assert!(crate::frame::Frame::from_qr_text(&codes[0]).is_err());
    }

    #[test]
    fn one_code_when_it_fits_and_errors_when_it_cannot() {
        let codes = split(1, KIND_ANSWER, 0, b"hello", 400).unwrap();
        assert_eq!(codes.len(), 1);
        assert_eq!(LinkPart::decode(&codes[0]).unwrap().payload, b"hello");
        assert_eq!(
            split(1, KIND_ANSWER, 0, &[0; 10], 20),
            Err(LinkError::TooLong)
        );
        assert_eq!(
            split(1, KIND_ANSWER, 0, &[0; 100_000], 60),
            Err(LinkError::TooLong)
        );
    }

    #[test]
    fn rejects_damage() {
        let code = &split(1, KIND_OFFER, 0, b"payload", 400).unwrap()[0];
        assert_eq!(LinkPart::decode("QSF1-ABC"), Err(LinkError::BadPrefix));
        assert_eq!(
            LinkPart::decode(&code[..code.len() - 3]),
            Err(LinkError::Malformed)
        );
    }
}
