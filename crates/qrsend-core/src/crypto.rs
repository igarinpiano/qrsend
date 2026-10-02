//! Device identities, age encryption and meta signatures (docs/PROTOCOL.md §9).

use std::io::{self, BufRead, Read, Write};
use std::iter;

use age::secrecy::ExposeSecret;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use crate::manifest::{MetaSignature, signed_message};

const ID_PREFIX: &str = "qrsend-id:1:";
const SECRET_HEADER: &str = "# QRSend device identity — keep this file private";
const FPR_CONTEXT: &[u8] = b"QRSEND-FPR-v1";

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid device ID: {0}")]
    BadId(&'static str),
    #[error("invalid identity file: {0}")]
    BadIdentity(&'static str),
    #[error("this transfer is not addressed to this device")]
    NotForUs,
    #[error("decryption failed: {0}")]
    Decrypt(String),
    #[error("encryption failed: {0}")]
    Encrypt(String),
    #[error("data exceeds the expected size")]
    TooLarge,
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut it = s.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hex = [it.next()?, it.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).ok()
}

/// The public half of a device: what other devices store as a "trusted device".
#[derive(Clone)]
pub struct DevicePublic {
    pub name: String,
    pub recipient: age::x25519::Recipient,
    pub verifying: VerifyingKey,
}

impl std::fmt::Debug for DevicePublic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DevicePublic({:?}, {})", self.name, self.fingerprint())
    }
}

impl PartialEq for DevicePublic {
    fn eq(&self, other: &Self) -> bool {
        self.recipient.to_string() == other.recipient.to_string()
            && self.verifying == other.verifying
    }
}

/// Human-comparable fingerprint of a signing key and recipient, `1234-5678-9012`.
pub fn fingerprint(recipient: &str, verifying: &[u8; 32]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(FPR_CONTEXT);
    h.update(recipient.as_bytes());
    h.update(verifying);
    let digest = h.finalize();
    let n = u64::from_le_bytes(digest.as_bytes()[..8].try_into().unwrap()) % 1_000_000_000_000;
    let s = format!("{n:012}");
    format!("{}-{}-{}", &s[..4], &s[4..8], &s[8..])
}

impl DevicePublic {
    pub fn fingerprint(&self) -> String {
        fingerprint(&self.recipient.to_string(), &self.verifying.to_bytes())
    }

    /// `qrsend-id:1:<age recipient>:<ed25519 base64url>:<name>`
    pub fn to_id_string(&self) -> String {
        format!(
            "{ID_PREFIX}{}:{}:{}",
            self.recipient,
            URL_SAFE_NO_PAD.encode(self.verifying.to_bytes()),
            percent_encode(&self.name)
        )
    }

    pub fn parse(id: &str) -> Result<DevicePublic, CryptoError> {
        let rest = id
            .trim()
            .strip_prefix(ID_PREFIX)
            .ok_or(CryptoError::BadId("missing qrsend-id:1: prefix"))?;
        let mut parts = rest.splitn(3, ':');
        let recipient = parts
            .next()
            .and_then(|r| r.parse::<age::x25519::Recipient>().ok())
            .ok_or(CryptoError::BadId("bad age recipient"))?;
        let key: [u8; 32] = parts
            .next()
            .and_then(|k| URL_SAFE_NO_PAD.decode(k).ok())
            .and_then(|k| k.try_into().ok())
            .ok_or(CryptoError::BadId("bad signing key"))?;
        let verifying =
            VerifyingKey::from_bytes(&key).map_err(|_| CryptoError::BadId("bad signing key"))?;
        let name = parts
            .next()
            .and_then(percent_decode)
            .ok_or(CryptoError::BadId("bad name"))?;
        Ok(DevicePublic {
            name,
            recipient,
            verifying,
        })
    }
}

/// This device's private keys.
pub struct DeviceIdentity {
    pub name: String,
    x25519: age::x25519::Identity,
    signing: SigningKey,
}

impl DeviceIdentity {
    pub fn generate(name: &str) -> Result<DeviceIdentity, CryptoError> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(DeviceIdentity {
            name: name.to_string(),
            x25519: age::x25519::Identity::generate(),
            signing: SigningKey::from_bytes(&seed),
        })
    }

    pub fn public(&self) -> DevicePublic {
        DevicePublic {
            name: self.name.clone(),
            recipient: self.x25519.to_public(),
            verifying: self.signing.verifying_key(),
        }
    }

    pub fn to_secret_string(&self) -> String {
        format!(
            "{SECRET_HEADER}\nname={}\nx25519={}\ned25519={}\n",
            percent_encode(&self.name),
            self.x25519.to_string().expose_secret(),
            URL_SAFE_NO_PAD.encode(self.signing.to_bytes())
        )
    }

    pub fn parse(text: &str) -> Result<DeviceIdentity, CryptoError> {
        let (mut name, mut x, mut ed) = (None, None, None);
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            match line.split_once('=') {
                Some(("name", v)) => name = percent_decode(v),
                Some(("x25519", v)) => x = v.parse::<age::x25519::Identity>().ok(),
                Some(("ed25519", v)) => {
                    ed = URL_SAFE_NO_PAD
                        .decode(v)
                        .ok()
                        .and_then(|b| <[u8; 32]>::try_from(b).ok())
                        .map(|b| SigningKey::from_bytes(&b))
                }
                _ => return Err(CryptoError::BadIdentity("unknown line")),
            }
        }
        Ok(DeviceIdentity {
            name: name.ok_or(CryptoError::BadIdentity("missing name"))?,
            x25519: x.ok_or(CryptoError::BadIdentity("missing x25519 key"))?,
            signing: ed.ok_or(CryptoError::BadIdentity("missing ed25519 key"))?,
        })
    }

    pub fn sign_meta(&self, session_id: u32, manifest_z: &[u8]) -> MetaSignature {
        let sig = self.signing.sign(&signed_message(session_id, manifest_z));
        MetaSignature {
            signer: self.signing.verifying_key().to_bytes(),
            signature: sig.to_bytes(),
        }
    }

    fn age_identity(&self) -> &dyn age::Identity {
        &self.x25519
    }
}

/// Checks a meta signature; `Ok(key)` is the signer's verified public key.
pub fn verify_meta(
    sig: &MetaSignature,
    session_id: u32,
    manifest_z: &[u8],
) -> Option<VerifyingKey> {
    let key = VerifyingKey::from_bytes(&sig.signer).ok()?;
    let signature = Signature::from_bytes(&sig.signature);
    key.verify_strict(&signed_message(session_id, manifest_z), &signature)
        .ok()
        .map(|_| key)
}

fn encryptor(recipients: &[DevicePublic]) -> Result<age::Encryptor, CryptoError> {
    age::Encryptor::with_recipients(
        recipients
            .iter()
            .map(|r| &r.recipient as &dyn age::Recipient),
    )
    .map_err(|e| CryptoError::Encrypt(e.to_string()))
}

/// Streaming age encryption to every recipient.
pub fn encrypt_writer<W: Write>(
    inner: W,
    recipients: &[DevicePublic],
) -> Result<age::stream::StreamWriter<W>, CryptoError> {
    Ok(encryptor(recipients)?.wrap_output(inner)?)
}

pub fn encrypt(data: &[u8], recipients: &[DevicePublic]) -> Result<Vec<u8>, CryptoError> {
    let mut w = encrypt_writer(Vec::new(), recipients)?;
    w.write_all(data)?;
    Ok(w.finish()?)
}

fn map_decrypt(e: age::DecryptError) -> CryptoError {
    match e {
        age::DecryptError::NoMatchingKeys => CryptoError::NotForUs,
        e => CryptoError::Decrypt(e.to_string()),
    }
}

/// Streaming age decryption.
pub fn decrypt_reader<R: BufRead>(
    inner: R,
    identity: &DeviceIdentity,
) -> Result<age::stream::StreamReader<R>, CryptoError> {
    let dec = age::Decryptor::new_buffered(inner).map_err(map_decrypt)?;
    dec.decrypt(iter::once(identity.age_identity()))
        .map_err(map_decrypt)
}

/// Decrypts a small object (the meta segment), refusing more than `limit` bytes.
pub fn decrypt(
    data: &[u8],
    identity: &DeviceIdentity,
    limit: usize,
) -> Result<Vec<u8>, CryptoError> {
    let mut out = Vec::new();
    decrypt_reader(data, identity)?
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| CryptoError::Decrypt(e.to_string()))?;
    if out.len() > limit {
        return Err(CryptoError::TooLarge);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_string_roundtrip() {
        let me = DeviceIdentity::generate("chimo's Mac: 開発機").unwrap();
        let id = me.public().to_id_string();
        assert!(id.starts_with("qrsend-id:1:age1"));
        let parsed = DevicePublic::parse(&id).unwrap();
        assert_eq!(parsed, me.public());
        assert_eq!(parsed.name, "chimo's Mac: 開発機");
        assert_eq!(parsed.fingerprint().len(), 14);
        assert!(DevicePublic::parse("qrsend-id:1:nope").is_err());
    }

    #[test]
    fn identity_file_roundtrip() {
        let me = DeviceIdentity::generate("box").unwrap();
        let back = DeviceIdentity::parse(&me.to_secret_string()).unwrap();
        assert_eq!(back.public(), me.public());
    }

    #[test]
    fn encrypt_to_several_recipients() {
        let (a, b, c) = (
            DeviceIdentity::generate("a").unwrap(),
            DeviceIdentity::generate("b").unwrap(),
            DeviceIdentity::generate("c").unwrap(),
        );
        let ct = encrypt(b"secret payload", &[a.public(), b.public()]).unwrap();
        assert_eq!(decrypt(&ct, &a, 100).unwrap(), b"secret payload");
        assert_eq!(decrypt(&ct, &b, 100).unwrap(), b"secret payload");
        assert!(matches!(decrypt(&ct, &c, 100), Err(CryptoError::NotForUs)));
        assert!(matches!(decrypt(&ct, &a, 5), Err(CryptoError::TooLarge)));
    }

    #[test]
    fn signatures() {
        let me = DeviceIdentity::generate("me").unwrap();
        let sig = me.sign_meta(42, b"manifest");
        assert_eq!(
            verify_meta(&sig, 42, b"manifest"),
            Some(me.public().verifying)
        );
        assert_eq!(verify_meta(&sig, 43, b"manifest"), None);
        assert_eq!(verify_meta(&sig, 42, b"tampered"), None);
    }
}
