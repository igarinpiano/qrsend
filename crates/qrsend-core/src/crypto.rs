//! Device identities, age encryption and meta signatures (docs/PROTOCOL.md §9).

use std::io::{self, BufRead, Read, Write};
use std::iter;

use age::secrecy::ExposeSecret;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::manifest::{MetaSignature, signed_message};
use crate::pq::{self, HybridIdentity, HybridRecipient, PqPublic, PqSecret};

const ID_PREFIX: &str = "qrsend-id:1:";
/// An ID with post-quantum keys: all four public keys as one base64url
/// string (X25519, Ed25519, ML-KEM-768, ML-DSA-65), then the name.
const ID_PREFIX_PQ: &str = "qrsend-id:2:";
const SECRET_HEADER: &str = "# QRSend device identity — keep this file private";
const FPR_CONTEXT: &[u8] = b"QRSEND-FPR-v1";
const FPR_CONTEXT_PQ: &[u8] = b"QRSEND-FPR-v2";

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
    /// The post-quantum keys, of a device that has them (PROTOCOL §9.1).
    /// With them, what is encrypted to the device and what it signs holds
    /// against a quantum computer as well.
    pub pq: Option<PqPublic>,
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
            && self.pq == other.pq
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

/// The fingerprint of a device with post-quantum keys, over all four keys:
/// 20 digits (about 66 bits; the 12 of the older kind were short).
fn fingerprint_pq(x25519: &[u8; 32], verifying: &[u8; 32], pq: &PqPublic) -> String {
    let mut h = blake3::Hasher::new();
    h.update(FPR_CONTEXT_PQ);
    h.update(x25519);
    h.update(verifying);
    h.update(&pq.kem);
    h.update(&pq.sig);
    let digest = h.finalize();
    let n = u128::from_le_bytes(digest.as_bytes()[..16].try_into().unwrap()) % 10u128.pow(20);
    let s = format!("{n:020}");
    (0..5)
        .map(|i| &s[i * 4..i * 4 + 4])
        .collect::<Vec<_>>()
        .join("-")
}

impl DevicePublic {
    /// The raw X25519 public key.
    pub fn x25519(&self) -> [u8; 32] {
        public_from_recipient(&self.recipient.to_string()).expect("an age recipient")
    }

    pub fn fingerprint(&self) -> String {
        match &self.pq {
            Some(pq) => fingerprint_pq(&self.x25519(), &self.verifying.to_bytes(), pq),
            None => fingerprint(&self.recipient.to_string(), &self.verifying.to_bytes()),
        }
    }

    /// The same device without its post-quantum keys: its ID as versions
    /// before them understand it (and as devices that paired with it then
    /// know it).
    pub fn classic(&self) -> DevicePublic {
        DevicePublic {
            pq: None,
            ..self.clone()
        }
    }

    /// Whether `signature` is this device's Ed25519 signature over
    /// `message` (the counterpart of `DeviceIdentity::sign_bytes`).
    pub fn verify_bytes(&self, message: &[u8], signature: &[u8]) -> bool {
        Signature::from_slice(signature).is_ok_and(|s| self.verifying.verify(message, &s).is_ok())
    }

    /// `qrsend-id:1:<age recipient>:<ed25519 base64url>:<name>`, or with
    /// post-quantum keys `qrsend-id:2:<all four keys, base64url>:<name>`.
    pub fn to_id_string(&self) -> String {
        match &self.pq {
            Some(pq) => {
                let mut keys = self.x25519().to_vec();
                keys.extend_from_slice(&self.verifying.to_bytes());
                keys.extend_from_slice(&pq.to_bytes());
                format!(
                    "{ID_PREFIX_PQ}{}:{}",
                    URL_SAFE_NO_PAD.encode(keys),
                    percent_encode(&self.name)
                )
            }
            None => format!(
                "{ID_PREFIX}{}:{}:{}",
                self.recipient,
                URL_SAFE_NO_PAD.encode(self.verifying.to_bytes()),
                percent_encode(&self.name)
            ),
        }
    }

    pub fn parse(id: &str) -> Result<DevicePublic, CryptoError> {
        let id = id.trim();
        if let Some(rest) = id.strip_prefix(ID_PREFIX_PQ) {
            let (keys, name) = rest.split_once(':').ok_or(CryptoError::BadId("bad keys"))?;
            let keys = URL_SAFE_NO_PAD
                .decode(keys)
                .map_err(|_| CryptoError::BadId("bad keys"))?;
            if keys.len() != 64 + pq::KEM_PUBLIC + pq::SIG_PUBLIC {
                return Err(CryptoError::BadId("bad keys"));
            }
            let name = percent_decode(name).ok_or(CryptoError::BadId("bad name"))?;
            let mut device = DevicePublic::from_raw(
                &name,
                keys[..32].try_into().unwrap(),
                keys[32..64].try_into().unwrap(),
            )?;
            device.pq =
                Some(PqPublic::from_bytes(&keys[64..]).ok_or(CryptoError::BadId("bad keys"))?);
            return Ok(device);
        }
        let rest = id
            .strip_prefix(ID_PREFIX)
            .ok_or(CryptoError::BadId("missing qrsend-id: prefix"))?;
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
            pq: None,
        })
    }
}

/// The first byte of an ID in bytes (see [`DevicePublic::to_bytes`]).
const ID_BYTES_PQ: u8 = 2;

impl DevicePublic {
    /// The ID of a device with post-quantum keys as bytes: what its codes
    /// carry (a marker, the four keys, the name, and four bytes of BLAKE3
    /// over all of that: the codes are read one by one, and parts of two
    /// IDs must not make a third). None for a device without those keys,
    /// whose ID is short enough to be shown as text.
    pub fn to_bytes(&self) -> Option<Vec<u8>> {
        let pq = self.pq.as_ref()?;
        let mut out = vec![ID_BYTES_PQ];
        out.extend_from_slice(&self.x25519());
        out.extend_from_slice(&self.verifying.to_bytes());
        out.extend_from_slice(&pq.to_bytes());
        out.extend_from_slice(self.name.as_bytes());
        let check = blake3::hash(&out);
        out.extend_from_slice(&check.as_bytes()[..4]);
        Some(out)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<DevicePublic, CryptoError> {
        let keys = 64 + pq::KEM_PUBLIC + pq::SIG_PUBLIC;
        if bytes.first() != Some(&ID_BYTES_PQ) || bytes.len() < 1 + keys + 4 {
            return Err(CryptoError::BadId("bad keys"));
        }
        let (bytes, check) = bytes.split_at(bytes.len() - 4);
        if blake3::hash(bytes).as_bytes()[..4] != *check {
            return Err(CryptoError::BadId("damaged ID"));
        }
        let (keys, name) = bytes[1..].split_at(keys);
        let name = std::str::from_utf8(name).map_err(|_| CryptoError::BadId("bad name"))?;
        let mut device = DevicePublic::from_raw(
            name,
            keys[..32].try_into().unwrap(),
            keys[32..64].try_into().unwrap(),
        )?;
        device.pq = Some(PqPublic::from_bytes(&keys[64..]).ok_or(CryptoError::BadId("bad keys"))?);
        Ok(device)
    }

    /// The codes this device's ID is shown as: one after the other, in any
    /// order and as often as it takes (each says which part it is). One
    /// code of text for a device without post-quantum keys, as ever;
    /// several of at most `max_chars` characters each for one with them.
    pub fn id_codes(&self, max_chars: usize) -> Result<Vec<String>, CryptoError> {
        let Some(bytes) = self.to_bytes() else {
            return Ok(vec![self.to_id_string()]);
        };
        let mut id = [0u8; 1];
        getrandom::getrandom(&mut id).map_err(|e| io::Error::other(e.to_string()))?;
        crate::link::split(0, crate::link::KIND_DEVICE_ID, id[0], &bytes, max_chars)
            .map_err(|_| CryptoError::BadId("the ID does not fit codes of this size"))
    }
}

/// Puts a device ID together from the codes it is shown as.
#[derive(Default)]
pub struct IdReader {
    assembler: crate::link::Assembler,
    /// Of the ID being read: its number, and which parts are here.
    parts: Option<(u8, Vec<bool>)>,
}

/// What a code read while looking for a device ID turned out to be.
#[derive(Debug, PartialEq)]
pub enum IdRead {
    /// The ID, whole.
    Whole(Box<DevicePublic>),
    /// A part of one: so many of so many are here.
    Part { have: usize, of: usize },
    /// Something else.
    Other,
}

impl IdReader {
    pub fn add(&mut self, text: &str) -> IdRead {
        let text = text.trim();
        if text.starts_with("qrsend-id:") {
            return DevicePublic::parse(text).map_or(IdRead::Other, |d| IdRead::Whole(Box::new(d)));
        }
        let Ok(part) = crate::link::LinkPart::decode(text) else {
            return IdRead::Other;
        };
        if part.kind != crate::link::KIND_DEVICE_ID || part.part >= part.parts {
            return IdRead::Other;
        }
        // (Another ID than the one being read: start over with it.)
        if !matches!(&self.parts, Some((id, parts)) if *id == part.id && parts.len() == part.parts as usize)
        {
            self.parts = Some((part.id, vec![false; part.parts as usize]));
        }
        let (_, parts) = self.parts.as_mut().expect("just set");
        parts[part.part as usize] = true;
        let progress = IdRead::Part {
            have: parts.iter().filter(|p| **p).count(),
            of: parts.len(),
        };
        match self.assembler.add(text) {
            Some(whole) => DevicePublic::from_bytes(&whole.payload)
                .map_or(IdRead::Other, |d| IdRead::Whole(Box::new(d))),
            None => progress,
        }
    }
}

/// This device's private keys.
pub struct DeviceIdentity {
    pub name: String,
    x25519: age::x25519::Identity,
    signing: SigningKey,
    /// The post-quantum keys (an identity made before them has none until
    /// it is [upgraded](DeviceIdentity::upgrade)), and what opens hybrid
    /// stanzas with them.
    pq: Option<(PqSecret, HybridIdentity)>,
    /// Open only what was encrypted with both schemes (see `require_pq`).
    pq_only: bool,
}

/// The raw secret of an age X25519 identity.
fn x25519_secret(identity: &age::x25519::Identity) -> [u8; 32] {
    let (_, secret) =
        bech32::decode(identity.to_string().expose_secret()).expect("age secret key is bech32");
    secret.try_into().expect("32-byte X25519 secret")
}

impl DeviceIdentity {
    pub fn generate(name: &str) -> Result<DeviceIdentity, CryptoError> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
        let mut identity = DeviceIdentity {
            name: name.to_string(),
            x25519: age::x25519::Identity::generate(),
            signing: SigningKey::from_bytes(&seed),
            pq: None,
            pq_only: false,
        };
        identity.upgrade()?;
        Ok(identity)
    }

    fn with_pq(&mut self, secret: PqSecret) {
        let hybrid = HybridIdentity::new(x25519_secret(&self.x25519), secret.clone());
        self.pq = Some((secret, hybrid));
    }

    /// Gives an identity made before them its post-quantum keys. The keys
    /// it has stay as they are: devices that know it go on as before, and
    /// use the new keys once they are given the new ID. False: it had them.
    pub fn upgrade(&mut self) -> Result<bool, CryptoError> {
        if self.pq.is_some() {
            return Ok(false);
        }
        let secret = PqSecret::generate().map_err(|e| io::Error::other(e.to_string()))?;
        self.with_pq(secret);
        Ok(true)
    }

    pub fn has_pq(&self) -> bool {
        self.pq.is_some()
    }

    /// From now on this identity opens only what was encrypted with both
    /// schemes: a transfer encrypted with X25519 alone (to this device's
    /// older ID) is, to it, not for this device.
    pub fn require_pq(&mut self) {
        self.pq_only = true;
    }

    pub fn public(&self) -> DevicePublic {
        DevicePublic {
            name: self.name.clone(),
            recipient: self.x25519.to_public(),
            verifying: self.signing.verifying_key(),
            pq: self.pq.as_ref().map(|(secret, _)| secret.public()),
        }
    }

    pub fn to_secret_string(&self) -> String {
        let mut text = format!(
            "{SECRET_HEADER}\nname={}\nx25519={}\ned25519={}\n",
            percent_encode(&self.name),
            self.x25519.to_string().expose_secret(),
            URL_SAFE_NO_PAD.encode(self.signing.to_bytes())
        );
        if let Some((secret, _)) = &self.pq {
            text.push_str(&format!(
                "mlkem768={}\nmldsa65={}\n",
                URL_SAFE_NO_PAD.encode(secret.kem_seed),
                URL_SAFE_NO_PAD.encode(secret.sig_seed)
            ));
        }
        text
    }

    pub fn parse(text: &str) -> Result<DeviceIdentity, CryptoError> {
        fn bytes<const N: usize>(v: &str) -> Option<[u8; N]> {
            URL_SAFE_NO_PAD.decode(v).ok()?.try_into().ok()
        }
        let (mut name, mut x, mut ed) = (None, None, None);
        let (mut kem, mut sig) = (None, None);
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            match line.split_once('=') {
                Some(("name", v)) => name = percent_decode(v),
                Some(("x25519", v)) => x = v.parse::<age::x25519::Identity>().ok(),
                Some(("ed25519", v)) => ed = bytes::<32>(v).map(|b| SigningKey::from_bytes(&b)),
                Some(("mlkem768", v)) => {
                    kem = Some(
                        bytes::<{ pq::KEM_SEED }>(v)
                            .ok_or(CryptoError::BadIdentity("bad ML-KEM key"))?,
                    )
                }
                Some(("mldsa65", v)) => {
                    sig = Some(
                        bytes::<{ pq::SIG_SEED }>(v)
                            .ok_or(CryptoError::BadIdentity("bad ML-DSA key"))?,
                    )
                }
                _ => return Err(CryptoError::BadIdentity("unknown line")),
            }
        }
        let mut identity = DeviceIdentity {
            name: name.ok_or(CryptoError::BadIdentity("missing name"))?,
            x25519: x.ok_or(CryptoError::BadIdentity("missing x25519 key"))?,
            signing: ed.ok_or(CryptoError::BadIdentity("missing ed25519 key"))?,
            pq: None,
            pq_only: false,
        };
        match (kem, sig) {
            (Some(kem_seed), Some(sig_seed)) => identity.with_pq(PqSecret { kem_seed, sig_seed }),
            (None, None) => {}
            // (Half of them is a damaged file, not an older one.)
            _ => {
                return Err(CryptoError::BadIdentity(
                    "one post-quantum key without the other",
                ));
            }
        }
        Ok(identity)
    }

    /// Signs a meta envelope: with Ed25519, and with ML-DSA as well by an
    /// identity that has post-quantum keys. Always both then: a receiver
    /// that knows those keys takes nothing less (see [`OpenedMeta::check`]).
    pub fn sign_meta(&self, session_id: u32, manifest_z: &[u8]) -> MetaSignature {
        let message = signed_message(session_id, manifest_z);
        let sig = self.signing.sign(&message);
        MetaSignature {
            signer: self.signing.verifying_key().to_bytes(),
            signature: sig.to_bytes(),
            pq: self.pq.as_ref().map(|(secret, _)| secret.sign(&message)),
        }
    }

    /// Ed25519 signature over arbitrary bytes (see [`signed_message`]).
    pub fn sign_bytes(&self, message: &[u8]) -> [u8; 64] {
        self.signing.sign(message).to_bytes()
    }

    /// The keys that decrypt transfers addressed to this device.
    pub fn as_age(&self) -> &dyn age::Identity {
        self
    }

    /// Raw key material, for importing into a platform key store (WebCrypto).
    pub fn raw_keys(&self) -> RawKeys {
        RawKeys {
            x25519_secret: x25519_secret(&self.x25519),
            x25519_public: public_from_recipient(&self.x25519.to_public().to_string())
                .expect("valid recipient"),
            ed25519_seed: self.signing.to_bytes(),
            ed25519_public: self.signing.verifying_key().to_bytes(),
            pq: self.pq.as_ref().map(|(secret, _)| secret.clone()),
        }
    }
}

/// Opens what was encrypted to this device: with both schemes where the
/// sender used both, with X25519 alone where it knew no other key.
impl age::Identity for DeviceIdentity {
    fn unwrap_stanza(
        &self,
        stanza: &age_core::format::Stanza,
    ) -> Option<Result<age_core::format::FileKey, age::DecryptError>> {
        if let Some((_, hybrid)) = &self.pq
            && let Some(opened) = hybrid.unwrap_stanza(stanza)
        {
            return Some(opened);
        }
        if self.pq_only {
            return None;
        }
        self.x25519.unwrap_stanza(stanza)
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
    pub pq: Option<PqSecret>,
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
            pq: None,
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
            // (A hybrid stanza has an X25519 half as well.)
            if let Some(epk) = pq::stanza_ephemeral(stanza) {
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

/// What keeps an encrypted transfer from being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Every receiver's key is wrapped with both schemes: X25519 and
    /// ML-KEM. It holds against a quantum computer as well.
    PostQuantum,
    /// At least one receiver's key is wrapped with X25519 alone, and
    /// whoever breaks that reads the transfer: all of it, since there is
    /// one key for everyone it was encrypted to.
    Older,
}

impl Protection {
    /// What a transfer to `recipients` gets.
    pub fn of(recipients: &[DevicePublic]) -> Protection {
        match recipients.iter().all(|r| r.pq.is_some()) {
            true => Protection::PostQuantum,
            false => Protection::Older,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Protection::PostQuantum => "pq",
            Protection::Older => "older",
        }
    }
}

/// What protects an age file, going by the stanzas of its header (which is
/// all of it that needs to be in `age_bytes`): readable without any key.
pub fn protection(age_bytes: &[u8]) -> Result<Protection, CryptoError> {
    struct Kinds(std::cell::Cell<(usize, usize)>);
    impl age::Identity for Kinds {
        fn unwrap_stanza(
            &self,
            stanza: &age_core::format::Stanza,
        ) -> Option<Result<age_core::format::FileKey, age::DecryptError>> {
            let (hybrid, other) = self.0.get();
            // (Stanzas put in to keep readers tolerant count for nothing.)
            if stanza.tag == pq::STANZA_TAG {
                self.0.set((hybrid + 1, other));
            } else if !stanza.tag.ends_with("-grease") {
                self.0.set((hybrid, other + 1));
            }
            None
        }
    }
    let kinds = Kinds(Default::default());
    let dec = age::Decryptor::new_buffered(age_bytes).map_err(map_decrypt)?;
    match dec.decrypt(iter::once(&kinds as &dyn age::Identity)) {
        Err(age::DecryptError::NoMatchingKeys) | Ok(_) => {}
        Err(e) => return Err(map_decrypt(e)),
    }
    Ok(match kinds.0.get() {
        (hybrid, 0) if hybrid > 0 => Protection::PostQuantum,
        _ => Protection::Older,
    })
}

/// An age identity made of precomputed X25519 shared secrets: the result of
/// `X25519(our secret, ephemeral key)` for each stanza, computed elsewhere.
pub struct SharedSecrets {
    pub our_public: [u8; 32],
    /// `(ephemeral public key, shared secret)` pairs.
    pub secrets: Vec<([u8; 32], [u8; 32])>,
    /// The post-quantum secret keys, for the stanzas made with both schemes
    /// (their X25519 half is among `secrets` like the others).
    pub pq: Option<PqSecret>,
}

impl age::Identity for SharedSecrets {
    fn unwrap_stanza(
        &self,
        stanza: &age_core::format::Stanza,
    ) -> Option<Result<age_core::format::FileKey, age::DecryptError>> {
        use age_core::format::{FILE_KEY_BYTES, FileKey};
        if let (Some(epk), Some(secret)) = (pq::stanza_ephemeral(stanza), &self.pq) {
            let (_, shared) = self.secrets.iter().find(|(e, _)| *e == epk)?;
            return pq::open_stanza(stanza, shared, &self.our_public, secret).map(Ok);
        }
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

/// To a device with post-quantum keys the file key is wrapped with both
/// schemes, and with both only: an X25519 stanza beside it would be a way
/// round the other. To a device without them, as ever.
fn encryptor(recipients: &[DevicePublic]) -> Result<age::Encryptor, CryptoError> {
    let wrapped: Vec<Box<dyn age::Recipient>> = recipients
        .iter()
        .map(|r| -> Result<Box<dyn age::Recipient>, CryptoError> {
            Ok(match &r.pq {
                Some(pq) => Box::new(
                    HybridRecipient::new(r.x25519(), pq)
                        .ok_or(CryptoError::BadId("bad ML-KEM key"))?,
                ),
                None => Box::new(r.recipient.clone()),
            })
        })
        .collect::<Result<_, _>>()?;
    age::Encryptor::with_recipients(wrapped.iter().map(|r| r.as_ref()))
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
    #[error(
        "the signature lacks the post-quantum half that this sender always gives — the transfer may be a forgery"
    )]
    Downgraded,
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
    /// The ML-DSA signature that came with it, and the bytes it is over.
    /// Not checked yet: its key is known only to whoever trusts the signer
    /// (see [`OpenedMeta::check`]).
    pq_signature: Option<Vec<u8>>,
    signed: Vec<u8>,
}

/// What the signatures of a transfer say about a device it claims to be from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signed {
    /// By this device, with every key it has.
    Yes,
    /// By another device, or not at all.
    No,
    /// With this device's Ed25519 key, but without (or with a wrong)
    /// signature of its post-quantum key. A device that has one always
    /// signs with both, so this is what a forgery by someone who broke the
    /// older scheme alone looks like: not to be taken for the device.
    Downgraded,
}

impl OpenedMeta {
    /// Whether the transfer is signed by `device`, a device this one trusts.
    pub fn check(&self, device: &DevicePublic) -> Signed {
        if self.signer != Some(device.verifying.to_bytes()) {
            return Signed::No;
        }
        match (&device.pq, &self.pq_signature) {
            (None, _) => Signed::Yes,
            (Some(pq), Some(signature)) if pq.verify(&self.signed, signature) => Signed::Yes,
            (Some(_), _) => Signed::Downgraded,
        }
    }
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
        pq_signature: env.signature.as_ref().and_then(|s| s.pq.clone()),
        signed: signed_message(session_id, &env.manifest_z),
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
        let id = me.public().classic().to_id_string();
        assert!(id.starts_with("qrsend-id:1:age1"));
        let parsed = DevicePublic::parse(&id).unwrap();
        assert_eq!(parsed, me.public().classic());
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
        assert_eq!(rebuilt, public.classic());
        assert_eq!(rebuilt.fingerprint(), public.classic().fingerprint());
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
            pq: raw.pq.clone(),
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
            pq: raw.pq.clone(),
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

    // ------------------------------------------------- post-quantum keys

    #[test]
    fn an_id_with_post_quantum_keys_reads_back() {
        let me = DeviceIdentity::generate("Mac — 仕事").unwrap();
        assert!(me.has_pq());
        let public = me.public();
        let id = public.to_id_string();
        assert!(id.starts_with("qrsend-id:2:"), "{id}");
        assert_eq!(DevicePublic::parse(&id).unwrap(), public);
        assert_eq!(DevicePublic::parse(&id).unwrap().name, "Mac — 仕事");
        // Five groups of four digits, over all the keys.
        let fingerprint = public.fingerprint();
        assert_eq!(fingerprint.len(), 24, "{fingerprint}");
        assert_ne!(fingerprint, public.classic().fingerprint());
        // The ID without them is the one older versions read.
        let classic = public.classic().to_id_string();
        assert!(classic.starts_with("qrsend-id:1:"));
        assert_eq!(DevicePublic::parse(&classic).unwrap(), public.classic());
        // Cut short or changed, it is no ID.
        assert!(DevicePublic::parse(&id[..id.len() - 40]).is_err());
        assert!(DevicePublic::parse(&id.replace("qrsend-id:2:", "qrsend-id:2:AAAA")).is_err());
        assert!(DevicePublic::parse("qrsend-id:2:").is_err());
        assert!(DevicePublic::parse("qrsend-id:3:x:y").is_err());
    }

    #[test]
    fn an_older_identity_is_given_the_new_keys_and_keeps_the_old() {
        let new = DeviceIdentity::generate("d").unwrap();
        // As a version before post-quantum keys wrote it: three lines.
        let text: String = new
            .to_secret_string()
            .lines()
            .filter(|l| !l.starts_with("mlkem768=") && !l.starts_with("mldsa65="))
            .map(|l| format!("{l}\n"))
            .collect();
        let mut old = DeviceIdentity::parse(&text).unwrap();
        assert!(!old.has_pq());
        assert_eq!(old.public(), new.public().classic());
        assert!(old.upgrade().unwrap());
        assert!(!old.upgrade().unwrap());
        assert_eq!(old.public().classic(), new.public().classic());
        // Written and read again, the keys are the same.
        let again = DeviceIdentity::parse(&old.to_secret_string()).unwrap();
        assert_eq!(again.public(), old.public());
        // One of the two new keys without the other is a damaged file.
        let half: String = old
            .to_secret_string()
            .lines()
            .filter(|l| !l.starts_with("mldsa65="))
            .map(|l| format!("{l}\n"))
            .collect();
        assert!(DeviceIdentity::parse(&half).is_err());
    }

    #[test]
    fn to_a_device_with_post_quantum_keys_both_schemes_are_used_and_only_both() {
        let bob = DeviceIdentity::generate("bob").unwrap();
        let ct = encrypt(b"for bob", &[bob.public()]).unwrap();
        assert_eq!(decrypt(&ct, bob.as_age(), 100).unwrap(), b"for bob");
        // (An X25519 stanza is some 100 bytes; the lattice one over 1400.)
        assert!(ct.len() > 1400, "{}", ct.len());
        // The same device with its older keys alone (X25519) does not open
        // it: there is no stanza that X25519 alone would open.
        let text: String = bob
            .to_secret_string()
            .lines()
            .filter(|l| !l.starts_with("mlkem768=") && !l.starts_with("mldsa65="))
            .map(|l| format!("{l}\n"))
            .collect();
        let bob_before = DeviceIdentity::parse(&text).unwrap();
        assert!(matches!(
            decrypt(&ct, bob_before.as_age(), 100),
            Err(CryptoError::NotForUs)
        ));
        // To the ID it had before, encryption is as it was, and it opens
        // that with or without the new keys.
        let ct = encrypt(b"as before", &[bob.public().classic()]).unwrap();
        assert!(ct.len() < 500);
        assert_eq!(decrypt(&ct, bob.as_age(), 100).unwrap(), b"as before");
        assert_eq!(
            decrypt(&ct, bob_before.as_age(), 100).unwrap(),
            b"as before"
        );
        // What protects each can be told without a key.
        let pq = encrypt(b"for bob", &[bob.public()]).unwrap();
        assert_eq!(protection(&pq).unwrap(), Protection::PostQuantum);
        assert_eq!(protection(&ct).unwrap(), Protection::Older);
        assert_eq!(Protection::of(&[bob.public()]), Protection::PostQuantum);
        assert_eq!(
            Protection::of(&[bob.public(), bob.public().classic()]),
            Protection::Older
        );
        assert!(protection(b"not an age file").is_err());
        // Several devices, of both kinds: each opens it, a stranger does not.
        let (carol, eve) = (
            DeviceIdentity::generate("carol").unwrap(),
            DeviceIdentity::generate("eve").unwrap(),
        );
        let to = [bob_before.public(), carol.public(), bob.public()];
        let ct = encrypt(b"for three", &to).unwrap();
        for device in [&bob_before, &carol, &bob] {
            assert_eq!(decrypt(&ct, device.as_age(), 100).unwrap(), b"for three");
        }
        // (One receiver with older keys only: that is what the whole is worth.)
        assert_eq!(protection(&ct).unwrap(), Protection::Older);
        assert!(matches!(
            decrypt(&ct, eve.as_age(), 100),
            Err(CryptoError::NotForUs)
        ));
    }

    #[test]
    fn a_device_with_post_quantum_keys_is_taken_only_with_both_signatures() {
        use crate::manifest::MetaEnvelope;
        let alice = DeviceIdentity::generate("alice").unwrap();
        let manifest = crate::manifest::tests::sample();
        let session = manifest.session_id().unwrap();
        let mut envelope = MetaEnvelope::from_manifest(&manifest).unwrap();
        let signature = alice.sign_meta(session, &envelope.manifest_z);
        assert_eq!(
            signature.pq.as_ref().map(Vec::len),
            Some(crate::pq::SIGNATURE)
        );
        envelope.signature = Some(signature.clone());
        let opened = open_meta(&envelope.encode(), session, false, None).unwrap();
        // Known with all its keys, known with the older ones only, and
        // another device altogether.
        assert_eq!(opened.check(&alice.public()), Signed::Yes);
        assert_eq!(opened.check(&alice.public().classic()), Signed::Yes);
        let other = DeviceIdentity::generate("other").unwrap();
        assert_eq!(opened.check(&other.public()), Signed::No);

        // Without the lattice signature (what someone who could forge
        // Ed25519 alone would send), or with a wrong one: not alice, to
        // whoever knows her post-quantum key.
        let mut classic_only = signature.clone();
        classic_only.pq = None;
        let mut wrong = signature.clone();
        wrong.pq.as_mut().unwrap()[10] ^= 1;
        for forged in [classic_only, wrong] {
            envelope.signature = Some(forged);
            let opened = open_meta(&envelope.encode(), session, false, None).unwrap();
            assert_eq!(opened.check(&alice.public()), Signed::Downgraded);
            // (Whoever knows only her older keys cannot tell.)
            assert_eq!(opened.check(&alice.public().classic()), Signed::Yes);
        }
        // The Ed25519 half is checked as ever.
        let mut bad = signature;
        bad.signature[0] ^= 1;
        envelope.signature = Some(bad);
        assert!(matches!(
            open_meta(&envelope.encode(), session, false, None),
            Err(OpenMetaError::BadSignature)
        ));
    }

    #[test]
    fn an_id_is_put_together_from_its_codes_in_any_order() {
        let public = DeviceIdentity::generate("phone 📱").unwrap().public();
        let codes = public.id_codes(600).unwrap();
        assert!((8..=10).contains(&codes.len()), "{}", codes.len());
        assert!(codes.iter().all(|c| c.len() <= 600));
        let mut reader = IdReader::default();
        // Out of order, with repeats, and with other codes in between.
        let mut order: Vec<usize> = (0..codes.len()).rev().collect();
        order.insert(2, 0);
        let mut got = None;
        for (n, i) in order.into_iter().enumerate() {
            assert_eq!(reader.add("QSL1-NOT-A-CODE"), IdRead::Other);
            match reader.add(&codes[i]) {
                IdRead::Whole(device) => got = Some(*device),
                IdRead::Part { have, of } => {
                    assert_eq!(of, codes.len());
                    assert!(have <= n + 1);
                }
                IdRead::Other => panic!("part {i} not taken"),
            }
        }
        assert_eq!(got, Some(public.clone()));
        // The text form is taken as it is, and an older ID is one code of text.
        let mut reader = IdReader::default();
        assert_eq!(
            reader.add(&public.to_id_string()),
            IdRead::Whole(Box::new(public.clone()))
        );
        let classic = public.classic().id_codes(600).unwrap();
        assert_eq!(classic, [public.classic().to_id_string()]);
        assert_eq!(
            reader.add(&classic[0]),
            IdRead::Whole(Box::new(public.classic()))
        );
        // The parts of one showing do not mix with those of another device.
        let other = DeviceIdentity::generate("other").unwrap().public();
        let others = other.id_codes(600).unwrap();
        let mut reader = IdReader::default();
        for code in codes.iter().take(codes.len() - 1) {
            reader.add(code);
        }
        let mut last = IdRead::Other;
        for code in &others {
            last = reader.add(code);
        }
        assert_eq!(last, IdRead::Whole(Box::new(other)));
    }
}
