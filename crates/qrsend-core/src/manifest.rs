//! Manifest JSON and the meta envelope that carries it. See docs/PROTOCOL.md §5.

use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: u32 = 1;
const META_MAGIC: &[u8; 3] = b"QSM";
const META_VERSION: u8 = 1;
const SIG_NONE: u8 = 0;
const SIG_ED25519: u8 = 1;
/// Domain separation prefix for meta signatures.
pub const SIGNATURE_CONTEXT: &[u8] = b"QRSEND-META-v1";
/// Upper bound on the decompressed manifest JSON, against decompression bombs.
pub const MAX_MANIFEST_JSON: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Files,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Encoding {
    Zstd,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryType {
    File,
    Dir,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: EntryType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtime: Option<i64>,
}

impl Entry {
    pub fn file_size(&self) -> u64 {
        self.size.unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub length: u64,
    pub blake3: String,
    pub segment_blake3: Vec<String>,
    pub encoding: Encoding,
    pub plain_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub qrsend: u32,
    pub session: String,
    pub kind: Kind,
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    pub body: Body,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

impl Manifest {
    pub fn session_id(&self) -> Option<u32> {
        u32::from_str_radix(&self.session, 16).ok()
    }

    pub fn file_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.kind == EntryType::File)
            .count()
    }

    /// Sum of declared file sizes; must equal `body.plain_length` for `files`.
    pub fn files_total(&self) -> u64 {
        self.entries.iter().map(Entry::file_size).sum()
    }
}

pub fn session_hex(session_id: u32) -> String {
    format!("{session_id:08x}")
}

/// Ed25519 signature carried in the meta envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaSignature {
    pub signer: [u8; 32],
    pub signature: [u8; 64],
}

/// Decoded meta envelope (§5.1), before the manifest is decompressed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaEnvelope {
    pub signature: Option<MetaSignature>,
    pub manifest_z: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("not a QRSend meta envelope")]
    BadMagic,
    #[error("unsupported meta version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported signature algorithm {0}")]
    UnsupportedSignature(u8),
    #[error("truncated meta envelope")]
    Truncated,
    #[error("manifest too large")]
    TooLarge,
    #[error("manifest session does not match frame session")]
    SessionMismatch,
    #[error("invalid manifest: {0}")]
    Invalid(String),
    #[error("manifest JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest decompression: {0}")]
    Io(#[from] std::io::Error),
}

/// Bytes covered by a meta signature.
pub fn signed_message(session_id: u32, manifest_z: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(SIGNATURE_CONTEXT.len() + 4 + manifest_z.len());
    m.extend_from_slice(SIGNATURE_CONTEXT);
    m.extend_from_slice(&session_id.to_le_bytes());
    m.extend_from_slice(manifest_z);
    m
}

impl MetaEnvelope {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + 96 + self.manifest_z.len());
        out.extend_from_slice(META_MAGIC);
        out.push(META_VERSION);
        match &self.signature {
            None => out.push(SIG_NONE),
            Some(sig) => {
                out.push(SIG_ED25519);
                out.extend_from_slice(&sig.signer);
                out.extend_from_slice(&sig.signature);
            }
        }
        out.extend_from_slice(&self.manifest_z);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetaError> {
        if bytes.len() < 5 {
            return Err(MetaError::Truncated);
        }
        if &bytes[..3] != META_MAGIC {
            return Err(MetaError::BadMagic);
        }
        if bytes[3] != META_VERSION {
            return Err(MetaError::UnsupportedVersion(bytes[3]));
        }
        let rest = &bytes[5..];
        match bytes[4] {
            SIG_NONE => Ok(MetaEnvelope {
                signature: None,
                manifest_z: rest.to_vec(),
            }),
            SIG_ED25519 => {
                if rest.len() < 96 {
                    return Err(MetaError::Truncated);
                }
                Ok(MetaEnvelope {
                    signature: Some(MetaSignature {
                        signer: rest[..32].try_into().unwrap(),
                        signature: rest[32..96].try_into().unwrap(),
                    }),
                    manifest_z: rest[96..].to_vec(),
                })
            }
            other => Err(MetaError::UnsupportedSignature(other)),
        }
    }

    /// Compresses `manifest` into an unsigned envelope.
    pub fn from_manifest(manifest: &Manifest) -> Result<Self, MetaError> {
        Ok(MetaEnvelope {
            signature: None,
            manifest_z: compress_manifest(manifest)?,
        })
    }

    /// Decompresses and validates the manifest against the frame session id.
    pub fn manifest(&self, session_id: u32) -> Result<Manifest, MetaError> {
        let json = crate::compress::decompress_bounded(&self.manifest_z, MAX_MANIFEST_JSON)
            .map_err(|e| match e {
                crate::compress::BoundedError::TooLarge => MetaError::TooLarge,
                crate::compress::BoundedError::Io(e) => MetaError::Io(e),
            })?;
        let manifest: Manifest = serde_json::from_slice(&json)?;
        if manifest.session_id() != Some(session_id) {
            return Err(MetaError::SessionMismatch);
        }
        validate(&manifest)?;
        Ok(manifest)
    }
}

pub fn compress_manifest(manifest: &Manifest) -> Result<Vec<u8>, MetaError> {
    let json = serde_json::to_vec(manifest)?;
    Ok(crate::compress::compress(&json, 19)?)
}

fn validate(m: &Manifest) -> Result<(), MetaError> {
    let bad = |s: &str| Err(MetaError::Invalid(s.to_string()));
    if m.qrsend != MANIFEST_VERSION {
        return bad("unsupported manifest version");
    }
    for e in &m.entries {
        match e.kind {
            EntryType::File if e.size.is_none() || e.blake3.is_none() => {
                return bad("file entry without size or hash");
            }
            EntryType::Dir if e.size.unwrap_or(0) != 0 => return bad("dir entry with size"),
            _ => {}
        }
    }
    match m.kind {
        Kind::Files if m.files_total() != m.body.plain_length => {
            bad("entry sizes do not add up to plain_length")
        }
        Kind::Text if !m.entries.is_empty() => bad("text transfer with entries"),
        _ => Ok(()),
    }
}

/// A session id that follows from what is sent: the same data, packed the
/// same way and described by the same manifest, gives the same id (and so
/// the same frames) every time.
///
/// A sender that stops and later sends the same data again then continues
/// the same session: a receiver that kept what it had needs only the rest,
/// with nothing for the sender to remember and no channel back to it. Only
/// for transfers whose bytes repeat, i.e. unencrypted ones (encryption uses
/// fresh keys every time). The manifest's own `session` field is left out of
/// the calculation; everything else in it counts, so a renamed file or
/// another sender name is another session.
pub fn content_session_id(manifest: &Manifest) -> u32 {
    let mut blank = manifest.clone();
    blank.session = String::new();
    let json = serde_json::to_vec(&blank).expect("a manifest serializes");
    let hash = blake3::derive_key("qrsend session id from content v1", &json);
    u32::from_le_bytes(hash[..4].try_into().unwrap()).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample() -> Manifest {
        Manifest {
            qrsend: 1,
            session: session_hex(0x1a2b3c4d),
            kind: Kind::Files,
            created: 1_790_000_000,
            sender_name: Some("test".into()),
            body: Body {
                length: 10,
                blake3: "00".into(),
                segment_blake3: vec!["00".into()],
                encoding: Encoding::Zstd,
                plain_length: 3,
            },
            entries: vec![
                Entry {
                    path: "d".into(),
                    kind: EntryType::Dir,
                    size: None,
                    blake3: None,
                    mode: None,
                    mtime: None,
                },
                Entry {
                    path: "d/a.txt".into(),
                    kind: EntryType::File,
                    size: Some(3),
                    blake3: Some("ab".into()),
                    mode: Some(0o644),
                    mtime: Some(1),
                },
            ],
        }
    }

    #[test]
    fn the_same_content_is_the_same_session() {
        let m = sample();
        let id = content_session_id(&m);
        assert_ne!(id, 0);
        assert_eq!(content_session_id(&sample()), id);
        // Whatever the manifest's own session field says.
        let relabeled = Manifest {
            session: session_hex(id),
            ..sample()
        };
        assert_eq!(content_session_id(&relabeled), id);
        // Anything else that differs makes another session.
        let mut renamed = sample();
        renamed.entries[1].path = "d/b.txt".into();
        assert_ne!(content_session_id(&renamed), id);
        let mut other_body = sample();
        other_body.body.blake3 = "01".into();
        assert_ne!(content_session_id(&other_body), id);
        let mut other_sender = sample();
        other_sender.sender_name = None;
        assert_ne!(content_session_id(&other_sender), id);
    }

    #[test]
    fn envelope_roundtrip() {
        let m = sample();
        let env = MetaEnvelope::from_manifest(&m).unwrap();
        let decoded = MetaEnvelope::decode(&env.encode()).unwrap();
        assert_eq!(decoded.manifest(0x1a2b3c4d).unwrap(), m);
        assert!(matches!(
            decoded.manifest(1),
            Err(MetaError::SessionMismatch)
        ));
    }

    #[test]
    fn signed_envelope_roundtrip() {
        let env = MetaEnvelope {
            signature: Some(MetaSignature {
                signer: [7; 32],
                signature: [9; 64],
            }),
            manifest_z: vec![1, 2, 3],
        };
        assert_eq!(MetaEnvelope::decode(&env.encode()).unwrap(), env);
    }

    #[test]
    fn rejects_inconsistent_sizes() {
        let mut m = sample();
        m.body.plain_length = 4;
        let env = MetaEnvelope::from_manifest(&m).unwrap();
        assert!(matches!(
            env.manifest(0x1a2b3c4d),
            Err(MetaError::Invalid(_))
        ));
    }
}
