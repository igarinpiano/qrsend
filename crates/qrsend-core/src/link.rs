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
/// The sender listens for a TCP connection (see [`TcpOffer`]); the receiver
/// connects, so no answer is needed.
pub const KIND_TCP_OFFER: u8 = 3;
/// A browser's offer to one device it has connected to before (§12.3).
pub const KIND_KNOWN_OFFER: u8 = 4;
/// An offer of a WebRTC connection to whoever reads it, made by a sender
/// that cannot be shown an answer: both sides work the answer out from the
/// offer (§12.4).
pub const KIND_OPEN_OFFER: u8 = 5;
/// A browser's offer to a receiver that cannot show it an answer (the
/// command-line program): the receiver's certificate follows from a seed in
/// the offer (§12.5, [`crate::linkcert`]).
pub const KIND_SEEDED_OFFER: u8 = 6;

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

/// A whole link message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkMessage {
    pub session_id: u32,
    pub kind: u8,
    pub id: u8,
    pub payload: Vec<u8>,
}

/// A message in the making: (session, kind, id, parts), and the parts seen.
type Pending = ((u32, u8, u8, u8), Vec<Option<Vec<u8>>>);

/// Puts link messages together from their codes, which arrive in any order
/// and repeatedly.
#[derive(Default)]
pub struct Assembler {
    pending: Vec<Pending>,
}

impl Assembler {
    /// Returns the whole message whenever `text` completes (or repeats a
    /// part of) one; `None` for anything else, other kinds of codes included.
    pub fn add(&mut self, text: &str) -> Option<LinkMessage> {
        let p = LinkPart::decode(text).ok()?;
        let key = (p.session_id, p.kind, p.id, p.parts);
        let at = match self.pending.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                if self.pending.len() >= 16 {
                    self.pending.remove(0);
                }
                self.pending.push((key, vec![None; p.parts as usize]));
                self.pending.len() - 1
            }
        };
        let parts = &mut self.pending[at].1;
        parts[p.part as usize] = Some(p.payload);
        let whole: Option<Vec<&Vec<u8>>> = parts.iter().map(Option::as_ref).collect();
        Some(LinkMessage {
            session_id: p.session_id,
            kind: p.kind,
            id: p.id,
            payload: whole?.into_iter().flatten().copied().collect(),
        })
    }
}

/// Payload of a [`KIND_TCP_OFFER`]: where the sender listens, and a key
/// that only those who see its screen know (see docs/PROTOCOL.md §12.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpOffer {
    pub port: u16,
    pub key: [u8; 16],
    /// Addresses to try, as text (IPv4 or IPv6).
    pub addresses: Vec<String>,
}

impl TcpOffer {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = self.port.to_be_bytes().to_vec();
        out.extend_from_slice(&self.key);
        out.push(self.addresses.len() as u8);
        for a in &self.addresses {
            out.push(a.len() as u8);
            out.extend_from_slice(a.as_bytes());
        }
        out
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, LinkError> {
        let mut rest = data;
        let mut take = |n: usize| {
            let (head, tail) = rest.split_at_checked(n).ok_or(LinkError::Malformed)?;
            rest = tail;
            Ok::<_, LinkError>(head)
        };
        let port = u16::from_be_bytes(take(2)?.try_into().unwrap());
        let key = take(16)?.try_into().unwrap();
        let count = take(1)?[0];
        let mut addresses = Vec::new();
        for _ in 0..count {
            let len = take(1)?[0] as usize;
            let a = std::str::from_utf8(take(len)?).map_err(|_| LinkError::Malformed)?;
            addresses.push(a.to_string());
        }
        Ok(TcpOffer {
            port,
            key,
            addresses,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembler_takes_parts_in_any_order() {
        let offer = TcpOffer {
            port: 40123,
            key: [7; 16],
            addresses: vec!["192.168.1.20".into(), "fd00::1".into()],
        };
        let codes = split(5, KIND_TCP_OFFER, 1, &offer.to_bytes(), 40).unwrap();
        assert!(codes.len() > 2);
        let mut a = Assembler::default();
        assert!(a.add("QSC1-ABC").is_none());
        let mut whole = None;
        for code in codes.iter().rev() {
            assert!(whole.is_none());
            whole = a.add(code);
        }
        let whole = whole.unwrap();
        assert_eq!(
            (whole.session_id, whole.kind, whole.id),
            (5, KIND_TCP_OFFER, 1)
        );
        assert_eq!(TcpOffer::from_bytes(&whole.payload).unwrap(), offer);
        // A repeated part gives the message again.
        assert!(a.add(&codes[0]).is_some());
        assert!(TcpOffer::from_bytes(&whole.payload[..10]).is_err());
    }

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
