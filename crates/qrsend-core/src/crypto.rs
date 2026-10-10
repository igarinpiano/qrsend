//! Device identities, age encryption and meta signatures (docs/PROTOCOL.md §9).

use std::io::{self, BufRead, Read, Write};
use std::iter;

use age::secrecy::ExposeSecret;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

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

    /// Whether `signature` is this device's Ed25519 signature over
    /// `message` (the counterpart of `DeviceIdentity::sign_bytes`).
    pub fn verify_bytes(&self, message: &[u8], signature: &[u8]) -> bool {
        Signature::from_slice(signature).is_ok_and(|s| self.verifying.verify(message, &s).is_ok())
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
        getrandom::getrandom(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
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

    /// Ed25519 signature over arbitrary bytes (see [`signed_message`]).
    pub fn sign_bytes(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// The key that decrypts transfers addressed to this device.
    pub fn as_age(&self) -> &dyn age::Identity {
        &self.x25519
    }

    /// Raw key material, for importing into a platform key store (WebCrypto).
    pub fn raw_keys(&self) -> RawKeys {
        let (_, secret) = bech32::decode(self.x25519.to_string().expose_secret())
            .expect("age secret key is bech32");
        RawKeys {
            x25519_secret: secret.try_into().expect("32-byte X25519 secret"),
            x25519_public: public_from_recipient(&self.x25519.to_public().to_string())
                .expect("valid recipient"),
            ed25519_seed: self.signing.to_bytes(),
            ed25519_public: self.signing.verifying_key().to_bytes(),
        }
    }
}

/// The age identity trait, for callers that do not depend on `age` directly.
pub use age::Identity as AgeIdentity;

/// The writer returned by [`encrypt_writer`]; call `finish()` when done.
pub type EncryptWriter<W> = age::stream::StreamWriter<W>;

/// Raw keys of a [`DeviceIdentity`]. Handle with care.
pub struct RawKeys {
    pub x25519_secret: [u8; 32],
    pub x25519_public: [u8; 32],
    pub ed25519_seed: [u8; 32],
    pub ed25519_public: [u8; 32],
}

/// The age recipient string (`age1…`) of a raw X25519 public key.
pub fn recipient_from_public(public: &[u8; 32]) -> String {
    age_core::primitives::bech32_encode(bech32::Hrp::parse_unchecked("age"), public)
}

/// The raw X25519 public key of an age recipient string.
pub fn public_from_recipient(recipient: &str) -> Option<[u8; 32]> {
    let (hrp, data) = bech32::decode(recipient).ok()?;
    (hrp.as_str() == "age").then_some(())?;
    data.try_into().ok()
}

impl DevicePublic {
    /// Builds the public half from raw keys (identities held in WebCrypto).
    pub fn from_raw(
        name: &str,
        x25519_public: &[u8; 32],
        ed25519_public: &[u8; 32],
    ) -> Result<Self, CryptoError> {
        Ok(DevicePublic {
            name: name.to_string(),
            recipient: recipient_from_public(x25519_public)
                .parse()
                .map_err(|_| CryptoError::BadId("bad X25519 key"))?,
            verifying: VerifyingKey::from_bytes(ed25519_public)
                .map_err(|_| CryptoError::BadId("bad signing key"))?,
        })
    }
}

const X25519_TAG: &str = "X25519";
const X25519_LABEL: &[u8] = b"age-encryption.org/v1/X25519";

/// The ephemeral public keys of the X25519 recipient stanzas of an age file
/// (only its header needs to be present in `age_bytes`).
///
/// A key holder that never reveals its secret (a non-extractable WebCrypto
/// key) computes one X25519 shared secret per returned key and then decrypts
/// with [`SharedSecrets`].
pub fn x25519_stanza_keys(age_bytes: &[u8]) -> Result<Vec<[u8; 32]>, CryptoError> {
    struct Collect(std::cell::RefCell<Vec<[u8; 32]>>);
    impl age::Identity for Collect {
        fn unwrap_stanza(
            &self,
            stanza: &age_core::format::Stanza,
        ) -> Option<Result<age_core::format::FileKey, age::DecryptError>> {
            if let (X25519_TAG, [arg]) = (stanza.tag.as_str(), &stanza.args[..])
                && let Ok(epk) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(arg)
                && let Ok(epk) = <[u8; 32]>::try_from(epk)
            {
                self.0.borrow_mut().push(epk);
            }
            None
        }
    }
    let collect = Collect(Default::default());
    let dec = age::Decryptor::new_buffered(age_bytes).map_err(map_decrypt)?;
    match dec.decrypt(iter::once(&collect as &dyn age::Identity)) {
        Err(age::DecryptError::NoMatchingKeys) | Ok(_) => Ok(collect.0.into_inner()),
        Err(e) => Err(map_decrypt(e)),
    }
}

/// An age identity made of precomputed X25519 shared secrets: the result of
/// `X25519(our secret, ephemeral key)` for each stanza, computed elsewhere.
pub struct SharedSecrets {
    pub our_public: [u8; 32],
    /// `(ephemeral public key, shared secret)` pairs.
    pub secrets: Vec<([u8; 32], [u8; 32])>,
}

impl age::Identity for SharedSecrets {
    fn unwrap_stanza(
        &self,
        stanza: &age_core::format::Stanza,
    ) -> Option<Result<age_core::format::FileKey, age::DecryptError>> {
        use age_core::format::{FILE_KEY_BYTES, FileKey};
        if stanza.tag != X25519_TAG {
            return None;
        }
        let [arg] = &stanza.args[..] else {
            return Some(Err(age::DecryptError::InvalidHeader));
        };
        let epk: [u8; 32] = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(arg)
            .ok()?
            .try_into()
            .ok()?;
        let (_, shared) = self.secrets.iter().find(|(e, _)| *e == epk)?;
        // An all-zero secret means a low-order point; reject like age does.
        if shared.iter().all(|&b| b == 0) {
            return Some(Err(age::DecryptError::InvalidHeader));
        }
        let mut salt = [0u8; 64];
        salt[..32].copy_from_slice(&epk);
        salt[32..].copy_from_slice(&self.our_public);
        let key = age_core::primitives::hkdf(&salt, X25519_LABEL, shared);
        // Not ours if it does not decrypt (another recipient's stanza).
        let plain = age_core::primitives::aead_decrypt(&key, FILE_KEY_BYTES, &stanza.body).ok()?;
        Some(Ok(FileKey::init_with_mut(|k| k.copy_from_slice(&plain))))
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
    identity: &dyn age::Identity,
) -> Result<age::stream::StreamReader<R>, CryptoError> {
    let dec = age::Decryptor::new_buffered(inner).map_err(map_decrypt)?;
    dec.decrypt(iter::once(identity)).map_err(map_decrypt)
}

/// Decrypts a small object (the meta segment), refusing more than `limit` bytes.
pub fn decrypt(
    data: &[u8],
    identity: &dyn age::Identity,
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

/// Upper bound on a decrypted meta segment.
pub const MAX_META: usize = 512 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum OpenMetaError {
    #[error("this transfer is encrypted, but this device has no identity yet")]
    NoIdentity,
    #[error("this transfer is encrypted for another device")]
    NotForUs,
    #[error("the sender's signature is invalid — the transfer may have been tampered with")]
    BadSignature,
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error(transparent)]
    Meta(#[from] crate::manifest::MetaError),
}

/// A decrypted, signature-checked manifest.
pub struct OpenedMeta {
    pub manifest: crate::manifest::Manifest,
    /// The verified signer's Ed25519 key, if the meta was signed.
    pub signer: Option<[u8; 32]>,
}

/// Decrypts (when `encrypted`) and verifies a meta segment.
pub fn open_meta(
    bytes: &[u8],
    session_id: u32,
    encrypted: bool,
    me: Option<&dyn age::Identity>,
) -> Result<OpenedMeta, OpenMetaError> {
    let plain = if encrypted {
        let me = me.ok_or(OpenMetaError::NoIdentity)?;
        match decrypt(bytes, me, MAX_META) {
            Err(CryptoError::NotForUs) => return Err(OpenMetaError::NotForUs),
            r => r?,
        }
    } else {
        bytes.to_vec()
    };
    let env = crate::manifest::MetaEnvelope::decode(&plain)?;
    let signer = match &env.signature {
        None => None,
        Some(sig) => Some(
            verify_meta(sig, session_id, &env.manifest_z)
                .ok_or(OpenMetaError::BadSignature)?
                .to_bytes(),
        ),
    };
    Ok(OpenedMeta {
        manifest: env.manifest(session_id)?,
        signer,
    })
}

/// Short display id of a signing key (for senders that are not trusted).
pub fn key_id(verifying: &[u8; 32]) -> String {
    blake3::hash(verifying).to_hex()[..12].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_signature_is_checked_against_its_id() {
        let me = DeviceIdentity::generate("me").unwrap();
        let other = DeviceIdentity::generate("other").unwrap();
        let signature = me.sign_bytes(b"this is my certificate");
        let id = DevicePublic::parse(&me.public().to_id_string()).unwrap();
        assert!(id.verify_bytes(b"this is my certificate", &signature));
        assert!(!id.verify_bytes(b"this is another one", &signature));
        assert!(
            !other
                .public()
                .verify_bytes(b"this is my certificate", &signature)
        );
        assert!(!id.verify_bytes(b"this is my certificate", &signature[..63]));
    }

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
        assert_eq!(decrypt(&ct, a.as_age(), 100).unwrap(), b"secret payload");
        assert_eq!(decrypt(&ct, b.as_age(), 100).unwrap(), b"secret payload");
        assert!(matches!(
            decrypt(&ct, c.as_age(), 100),
            Err(CryptoError::NotForUs)
        ));
        assert!(matches!(
            decrypt(&ct, a.as_age(), 5),
            Err(CryptoError::TooLarge)
        ));
    }

    #[test]
    fn raw_keys_and_recipient_strings() {
        let me = DeviceIdentity::generate("me").unwrap();
        let raw = me.raw_keys();
        let public = me.public();
        assert_eq!(
            recipient_from_public(&raw.x25519_public),
            public.recipient.to_string()
        );
        assert_eq!(
            public_from_recipient(&public.recipient.to_string()),
            Some(raw.x25519_public)
        );
        assert_eq!(public_from_recipient("age1nonsense"), None);
        let rebuilt =
            DevicePublic::from_raw("me", &raw.x25519_public, &raw.ed25519_public).unwrap();
        assert_eq!(rebuilt, public);
        assert_eq!(rebuilt.fingerprint(), public.fingerprint());
        // The raw secret really is the key: derive the public half from it.
        let derived =
            x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(raw.x25519_secret));
        assert_eq!(derived.to_bytes(), raw.x25519_public);
    }

    /// The WebCrypto flow: the secret never enters this crate; only the
    /// shared secrets computed with it do.
    #[test]
    fn decrypt_with_precomputed_shared_secrets() {
        let (me, other) = (
            DeviceIdentity::generate("me").unwrap(),
            DeviceIdentity::generate("other").unwrap(),
        );
        let ct = encrypt(b"for me and other", &[other.public(), me.public()]).unwrap();
        let epks = x25519_stanza_keys(&ct).unwrap();
        assert_eq!(epks.len(), 2);

        let raw = me.raw_keys();
        let secret = x25519_dalek::StaticSecret::from(raw.x25519_secret);
        let shared = SharedSecrets {
            our_public: raw.x25519_public,
            secrets: epks
                .iter()
                .map(|epk| {
                    (
                        *epk,
                        secret
                            .diffie_hellman(&x25519_dalek::PublicKey::from(*epk))
                            .to_bytes(),
                    )
                })
                .collect(),
        };
        assert_eq!(decrypt(&ct, &shared, 100).unwrap(), b"for me and other");

        // Someone else's shared secrets do not open it.
        let stranger = x25519_dalek::StaticSecret::from([7u8; 32]);
        let wrong = SharedSecrets {
            our_public: x25519_dalek::PublicKey::from(&stranger).to_bytes(),
            secrets: epks
                .iter()
                .map(|epk| {
                    (
                        *epk,
                        stranger
                            .diffie_hellman(&x25519_dalek::PublicKey::from(*epk))
                            .to_bytes(),
                    )
                })
                .collect(),
        };
        assert!(matches!(
            decrypt(&ct, &wrong, 100),
            Err(CryptoError::NotForUs)
        ));
        // The header alone is enough to list the stanzas.
        assert_eq!(x25519_stanza_keys(&ct[..ct.len() - 20]).unwrap(), epks);
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
