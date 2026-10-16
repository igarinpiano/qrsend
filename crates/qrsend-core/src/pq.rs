//! The post-quantum halves of a device's keys (docs/PROTOCOL.md §9.1,
//! docs/POST-QUANTUM.md): ML-KEM-768 beside X25519 for the key exchange,
//! ML-DSA-65 beside Ed25519 for signatures. Always both together (hybrid):
//! whoever would read or forge has to break the old scheme and the new one.
//!
//! The codes of a transfer are shown on a screen, and a screen can be
//! recorded: what is encrypted today has to hold against a computer of
//! tomorrow. The lattice schemes come from the RustCrypto crates (`ml-kem`,
//! `ml-dsa`); nothing of them is implemented here, only how the two shared
//! secrets become one key and how that travels in an age header.

use std::collections::HashSet;

use age_core::format::{FILE_KEY_BYTES, FileKey, Stanza};
use age_core::primitives::{aead_decrypt, aead_encrypt};
use age_core::secrecy::ExposeSecret;
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use ml_dsa::{Keypair, MlDsa65};
use ml_kem::kem::Decapsulate;
use ml_kem::{EncapsulateDeterministic, EncodedSizeUser, KemCore, MlKem768};
use sha3::{Digest, Sha3_256};
use x25519_dalek::{PublicKey, StaticSecret};

/// Sizes in bytes (FIPS 203, FIPS 204).
pub const KEM_PUBLIC: usize = 1184;
pub const KEM_CIPHERTEXT: usize = 1088;
pub const KEM_SEED: usize = 64;
pub const SIG_PUBLIC: usize = 1952;
pub const SIG_SEED: usize = 32;
pub const SIGNATURE: usize = crate::manifest::PQ_SIGNATURE;

/// The tag of the stanza that wraps a file key for a hybrid recipient.
pub const STANZA_TAG: &str = "qrsend-pq1";
/// What the two shared secrets are hashed with into the key that wraps the
/// file key: the combiner of X-Wing, under a label of its own (the keys are
/// not made the way X-Wing makes them, so this is not X-Wing).
const COMBINER_LABEL: &[u8] = b"qrsend-pq1 x25519+mlkem768";
/// The context of an ML-DSA signature over a meta envelope.
const SIGNATURE_CONTEXT: &[u8] = b"qrsend meta";

type Ek = <MlKem768 as KemCore>::EncapsulationKey;
type Dk = <MlKem768 as KemCore>::DecapsulationKey;

impl std::fmt::Debug for PqPublic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let digest = blake3::hash(&self.to_bytes());
        write!(f, "PqPublic({}…)", &digest.to_hex()[..16])
    }
}

/// The tag of the stanza for a recipient that also showed a one-time key
/// (forward secrecy): the file key is wrapped with the device's lasting
/// keys and with the one-time ones, and takes both to open.
pub const STANZA_TAG_ONCE: &str = "qrsend-pq2";
const COMBINER_LABEL_ONCE: &[u8] = b"qrsend-pq2 x25519+mlkem768, lasting+one-time";
/// Bytes of a one-time key's public and secret halves.
pub const ONCE_PUBLIC: usize = 32 + KEM_PUBLIC;
pub const ONCE_SECRET: usize = 32 + KEM_SEED;

/// The public half of a one-time key: an X25519 key and an ML-KEM-768 key
/// made for one transfer and thrown away after it. What was encrypted to
/// them cannot be read any more once they are gone, even by whoever later
/// gets hold of the device's lasting keys.
#[derive(Clone, PartialEq, Eq)]
pub struct OncePublic {
    pub x25519: [u8; 32],
    pub kem: Vec<u8>,
}

impl OncePublic {
    pub fn to_bytes(&self) -> Vec<u8> {
        [&self.x25519[..], &self.kem[..]].concat()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<OncePublic> {
        if bytes.len() != ONCE_PUBLIC {
            return None;
        }
        let (x25519, kem) = bytes.split_at(32);
        let ek = encapsulation_key(kem)?;
        (ek.as_bytes().as_slice() == kem).then(|| OncePublic {
            x25519: x25519.try_into().unwrap(),
            kem: kem.to_vec(),
        })
    }

    /// What a stanza names the key by.
    pub fn id(&self) -> [u8; 8] {
        blake3::hash(&self.to_bytes()).as_bytes()[..8]
            .try_into()
            .unwrap()
    }
}

/// The secret half of a one-time key, as the bytes it grows from.
#[derive(Clone)]
pub struct OnceSecret {
    pub x25519: [u8; 32],
    pub kem_seed: [u8; KEM_SEED],
}

impl OnceSecret {
    pub fn generate() -> Result<OnceSecret, getrandom::Error> {
        let mut secret = OnceSecret {
            x25519: [0; 32],
            kem_seed: [0; KEM_SEED],
        };
        getrandom::getrandom(&mut secret.x25519)?;
        getrandom::getrandom(&mut secret.kem_seed)?;
        Ok(secret)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        [&self.x25519[..], &self.kem_seed[..]].concat()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<OnceSecret> {
        let (x25519, kem_seed) = bytes.split_at_checked(32)?;
        Some(OnceSecret {
            x25519: x25519.try_into().ok()?,
            kem_seed: kem_seed.try_into().ok()?,
        })
    }

    fn kem(&self) -> (Dk, Ek) {
        PqSecret {
            kem_seed: self.kem_seed,
            sig_seed: [0; SIG_SEED],
        }
        .kem()
    }

    pub fn public(&self) -> OncePublic {
        OncePublic {
            x25519: PublicKey::from(&StaticSecret::from(self.x25519)).to_bytes(),
            kem: self.kem().1.as_bytes().to_vec(),
        }
    }
}

/// The post-quantum public keys of a device.
#[derive(Clone, PartialEq, Eq)]
pub struct PqPublic {
    /// ML-KEM-768 encapsulation key ([`KEM_PUBLIC`] bytes).
    pub kem: Vec<u8>,
    /// ML-DSA-65 verifying key ([`SIG_PUBLIC`] bytes).
    pub sig: Vec<u8>,
}

impl PqPublic {
    /// From the two keys one after the other; none if the sizes are wrong
    /// or the encapsulation key is not one (FIPS 203 §7.2: its numbers must
    /// be reduced).
    pub fn from_bytes(bytes: &[u8]) -> Option<PqPublic> {
        if bytes.len() != KEM_PUBLIC + SIG_PUBLIC {
            return None;
        }
        let (kem, sig) = bytes.split_at(KEM_PUBLIC);
        let ek = encapsulation_key(kem)?;
        // (Decoding takes every value below 2^12; encoding again gives the
        // same bytes only for values below the modulus.)
        (ek.as_bytes().as_slice() == kem).then(|| PqPublic {
            kem: kem.to_vec(),
            sig: sig.to_vec(),
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        [self.kem.as_slice(), self.sig.as_slice()].concat()
    }

    /// Whether `signature` is this device's ML-DSA signature over `message`.
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        let (Ok(key), Ok(signature)) = (
            ml_dsa::EncodedVerifyingKey::<MlDsa65>::try_from(self.sig.as_slice()),
            ml_dsa::EncodedSignature::<MlDsa65>::try_from(signature),
        ) else {
            return false;
        };
        let key = ml_dsa::VerifyingKey::<MlDsa65>::decode(&key);
        ml_dsa::Signature::<MlDsa65>::decode(&signature)
            .is_some_and(|s| key.verify_with_context(message, SIGNATURE_CONTEXT, &s))
    }
}

fn encapsulation_key(bytes: &[u8]) -> Option<Ek> {
    Some(Ek::from_bytes(&bytes.try_into().ok()?))
}

/// The post-quantum secret keys of a device, as the seeds they grow from.
#[derive(Clone)]
pub struct PqSecret {
    pub kem_seed: [u8; KEM_SEED],
    pub sig_seed: [u8; SIG_SEED],
}

impl PqSecret {
    pub fn generate() -> Result<PqSecret, getrandom::Error> {
        let mut secret = PqSecret {
            kem_seed: [0; KEM_SEED],
            sig_seed: [0; SIG_SEED],
        };
        getrandom::getrandom(&mut secret.kem_seed)?;
        getrandom::getrandom(&mut secret.sig_seed)?;
        Ok(secret)
    }

    fn kem(&self) -> (Dk, Ek) {
        let (d, z) = self.kem_seed.split_at(32);
        let (d, z): ([u8; 32], [u8; 32]) = (d.try_into().unwrap(), z.try_into().unwrap());
        MlKem768::generate_deterministic(&d.into(), &z.into())
    }

    fn signing(&self) -> ml_dsa::SigningKey<MlDsa65> {
        ml_dsa::SigningKey::<MlDsa65>::from_seed(&self.sig_seed.into())
    }

    pub fn public(&self) -> PqPublic {
        PqPublic {
            kem: self.kem().1.as_bytes().to_vec(),
            sig: self.signing().verifying_key().encode().to_vec(),
        }
    }

    /// ML-DSA signature over `message` (deterministic: the same message
    /// gives the same signature, and no randomness is needed to sign).
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.signing()
            .expanded_key()
            .sign_deterministic(message, SIGNATURE_CONTEXT)
            .expect("the context is short")
            .encode()
            .to_vec()
    }
}

/// The key that wraps a file key: both shared secrets, and what ties the
/// X25519 one to this exchange (ML-KEM's secret is tied to its ciphertext
/// and key by itself).
fn wrap_key(
    ss_kem: &[u8],
    ss_x: &[u8; 32],
    ephemeral: &[u8; 32],
    recipient: &[u8; 32],
) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update(ss_kem);
    h.update(ss_x);
    h.update(ephemeral);
    h.update(recipient);
    h.update(COMBINER_LABEL);
    h.finalize().into()
}

/// A device that is encrypted to with both schemes.
pub struct HybridRecipient {
    x25519: [u8; 32],
    kem: Ek,
    /// The one-time key it showed for this transfer, if it did.
    once: Option<(OncePublic, Ek)>,
}

impl HybridRecipient {
    /// `x25519`: the device's X25519 public key; `pq`: its other keys.
    pub fn new(x25519: [u8; 32], pq: &PqPublic) -> Option<HybridRecipient> {
        Some(HybridRecipient {
            x25519,
            kem: encapsulation_key(&pq.kem)?,
            once: None,
        })
    }

    /// With forward secrecy: the file key takes the one-time key as well.
    pub fn with_once(mut self, once: &OncePublic) -> Option<HybridRecipient> {
        self.once = Some((once.clone(), encapsulation_key(&once.kem)?));
        Some(self)
    }
}

impl age::Recipient for HybridRecipient {
    fn wrap_file_key(
        &self,
        file_key: &FileKey,
    ) -> Result<(Vec<Stanza>, HashSet<String>), age::EncryptError> {
        let failed = |what: &str| age::EncryptError::Io(std::io::Error::other(what.to_string()));
        let mut random = [0u8; 96];
        getrandom::getrandom(&mut random).map_err(|_| failed("no randomness"))?;
        let ephemeral = StaticSecret::from(<[u8; 32]>::try_from(&random[..32]).unwrap());
        let ephemeral_public = PublicKey::from(&ephemeral).to_bytes();
        // (A key that is no point of the curve at all gives nothing to
        // agree on.)
        let agree = |with: &[u8; 32]| {
            let shared = ephemeral.diffie_hellman(&PublicKey::from(*with));
            shared
                .was_contributory()
                .then(|| shared.to_bytes())
                .ok_or_else(|| failed("the recipient's X25519 key is not usable"))
        };
        let encapsulate = |to: &Ek, random: &[u8]| {
            let message: [u8; 32] = random.try_into().unwrap();
            to.encapsulate_deterministic(&message.into())
                .map_err(|_| failed("ML-KEM"))
        };
        let ss_x = agree(&self.x25519)?;
        let (ciphertext, ss_kem) = encapsulate(&self.kem, &random[32..64])?;
        let mut body = ciphertext.to_vec();
        let (tag, args, key) = match &self.once {
            None => (
                STANZA_TAG,
                vec![STANDARD_NO_PAD.encode(ephemeral_public)],
                wrap_key(&ss_kem, &ss_x, &ephemeral_public, &self.x25519),
            ),
            Some((once, once_kem)) => {
                let once_x = agree(&once.x25519)?;
                let (once_ciphertext, once_ss) = encapsulate(once_kem, &random[64..])?;
                body.extend_from_slice(&once_ciphertext);
                (
                    STANZA_TAG_ONCE,
                    vec![
                        STANDARD_NO_PAD.encode(ephemeral_public),
                        STANDARD_NO_PAD.encode(once.id()),
                    ],
                    wrap_key_once(
                        &wrap_key(&ss_kem, &ss_x, &ephemeral_public, &self.x25519),
                        &once_ss,
                        &once_x,
                        &once.x25519,
                    ),
                )
            }
        };
        body.extend_from_slice(&aead_encrypt(&key, file_key.expose_secret()));
        Ok((
            vec![Stanza {
                tag: tag.to_owned(),
                args,
                body,
            }],
            HashSet::new(),
        ))
    }
}

/// The key that wraps a file key for a recipient with a one-time key: the
/// key for its lasting keys (see `wrap_key`), and what was agreed with the
/// one-time ones. Whoever lacks either cannot make it.
fn wrap_key_once(
    lasting: &[u8; 32],
    ss_kem: &[u8],
    ss_x: &[u8; 32],
    once_x: &[u8; 32],
) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update(lasting);
    h.update(ss_kem);
    h.update(ss_x);
    h.update(once_x);
    h.update(COMBINER_LABEL_ONCE);
    h.finalize().into()
}

/// The one-time key a stanza was made for (its name), if it is of that kind.
pub fn stanza_once(stanza: &Stanza) -> Option<[u8; 8]> {
    if stanza.tag != STANZA_TAG_ONCE || stanza.args.len() != 2 {
        return None;
    }
    STANDARD_NO_PAD
        .decode(&stanza.args[1])
        .ok()?
        .try_into()
        .ok()
}

/// The ephemeral X25519 key of a hybrid stanza, if `stanza` is one.
pub fn stanza_ephemeral(stanza: &Stanza) -> Option<[u8; 32]> {
    let ciphertexts = match (stanza.tag.as_str(), stanza.args.len()) {
        (STANZA_TAG, 1) => 1,
        (STANZA_TAG_ONCE, 2) => 2,
        _ => return None,
    };
    if stanza.body.len() != ciphertexts * KEM_CIPHERTEXT + FILE_KEY_BYTES + 16 {
        return None;
    }
    STANDARD_NO_PAD
        .decode(&stanza.args[0])
        .ok()?
        .try_into()
        .ok()
}

/// Opens a hybrid stanza. `ss_x`: the X25519 secret shared with the
/// stanza's ephemeral key, computed by whoever holds this device's X25519
/// secret (which may be a key store that never gives it out); `recipient`:
/// this device's X25519 public key. None: the stanza is of another kind, or
/// was made for another device (a file for several devices holds one for
/// each), or was changed on the way.
pub fn open_stanza(
    stanza: &Stanza,
    ss_x: &[u8; 32],
    recipient: &[u8; 32],
    secret: &PqSecret,
    once: &[OnceSecret],
) -> Option<FileKey> {
    let ephemeral = stanza_ephemeral(stanza)?;
    let decapsulate = |dk: &Dk, ciphertext: &[u8]| {
        let ciphertext = ml_kem::Ciphertext::<MlKem768>::try_from(ciphertext).ok()?;
        // (ML-KEM never fails to give a secret: for a ciphertext made for
        // another key it gives one that opens nothing.)
        dk.decapsulate(&ciphertext).ok()
    };
    let (ciphertext, rest) = stanza.body.split_at(KEM_CIPHERTEXT);
    let ss_kem = decapsulate(&secret.kem().0, ciphertext)?;
    let mut key = wrap_key(&ss_kem, ss_x, &ephemeral, recipient);
    let mut wrapped = rest;
    // Made for a one-time key as well: one of those held here, or none.
    if let Some(id) = stanza_once(stanza) {
        let once = once.iter().find(|o| o.public().id() == id)?;
        let (ciphertext, rest) = rest.split_at(KEM_CIPHERTEXT);
        let once_ss = decapsulate(&once.kem().0, ciphertext)?;
        let once_x = StaticSecret::from(once.x25519).diffie_hellman(&PublicKey::from(ephemeral));
        key = wrap_key_once(&key, &once_ss, once_x.as_bytes(), &once.public().x25519);
        wrapped = rest;
    }
    let opened = aead_decrypt(&key, FILE_KEY_BYTES, wrapped).ok()?;
    Some(FileKey::init_with_mut(|key| key.copy_from_slice(&opened)))
}

/// The keys that open hybrid stanzas, on a device that holds its X25519
/// secret itself.
pub struct HybridIdentity {
    x25519: StaticSecret,
    secret: PqSecret,
    /// The one-time keys shown and not yet done with.
    pub once: Vec<OnceSecret>,
}

impl HybridIdentity {
    pub fn new(x25519_secret: [u8; 32], secret: PqSecret) -> HybridIdentity {
        HybridIdentity {
            x25519: StaticSecret::from(x25519_secret),
            secret,
            once: Vec::new(),
        }
    }
}

impl age::Identity for HybridIdentity {
    fn unwrap_stanza(&self, stanza: &Stanza) -> Option<Result<FileKey, age::DecryptError>> {
        let ephemeral = stanza_ephemeral(stanza)?;
        let ss_x = self.x25519.diffie_hellman(&PublicKey::from(ephemeral));
        let recipient = PublicKey::from(&self.x25519).to_bytes();
        open_stanza(
            stanza,
            ss_x.as_bytes(),
            &recipient,
            &self.secret,
            &self.once,
        )
        .map(Ok)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use age::{Identity, Recipient};

    fn device() -> ([u8; 32], [u8; 32], PqSecret) {
        let mut x = [0u8; 32];
        getrandom::getrandom(&mut x).unwrap();
        let public = PublicKey::from(&StaticSecret::from(x)).to_bytes();
        (x, public, PqSecret::generate().unwrap())
    }

    fn copy(stanza: &Stanza) -> Stanza {
        Stanza {
            tag: stanza.tag.clone(),
            args: stanza.args.clone(),
            body: stanza.body.clone(),
        }
    }

    fn file_key() -> FileKey {
        FileKey::init_with_mut(|k| getrandom::getrandom(k).unwrap())
    }

    #[test]
    fn sizes_are_those_of_the_standards() {
        let public = PqSecret::generate().unwrap().public();
        assert_eq!(
            (public.kem.len(), public.sig.len()),
            (KEM_PUBLIC, SIG_PUBLIC)
        );
        let secret = PqSecret::generate().unwrap();
        assert_eq!(secret.sign(b"x").len(), SIGNATURE);
        assert_eq!(PqPublic::from_bytes(&public.to_bytes()), Some(public));
    }

    #[test]
    fn a_file_key_wrapped_for_a_device_is_opened_by_it_alone() {
        let (x, x_public, secret) = device();
        let key = file_key();
        let to = HybridRecipient::new(x_public, &secret.public()).unwrap();
        let (stanzas, labels) = to.wrap_file_key(&key).unwrap();
        assert!(labels.is_empty() && stanzas.len() == 1);
        assert_eq!(stanzas[0].tag, STANZA_TAG);
        assert_eq!(stanzas[0].body.len(), KEM_CIPHERTEXT + 32);

        let own = HybridIdentity::new(x, secret.clone());
        let opened = own.unwrap_stanza(&stanzas[0]).unwrap().unwrap();
        assert_eq!(opened.expose_secret(), key.expose_secret());

        // Either half alone is not enough: the right lattice key with
        // another X25519 key, and the other way round.
        let (other_x, _, other_secret) = device();
        for wrong in [
            HybridIdentity::new(other_x, secret.clone()),
            HybridIdentity::new(x, other_secret),
        ] {
            assert!(wrong.unwrap_stanza(&stanzas[0]).is_none());
        }
        // Changed on the way, anywhere: not opened.
        for at in [
            0,
            500,
            KEM_CIPHERTEXT - 1,
            KEM_CIPHERTEXT,
            KEM_CIPHERTEXT + 31,
        ] {
            let mut damaged = copy(&stanzas[0]);
            damaged.body[at] ^= 1;
            assert!(own.unwrap_stanza(&damaged).is_none(), "{at}");
        }
        let mut damaged = copy(&stanzas[0]);
        damaged.args[0] = STANDARD_NO_PAD.encode([7u8; 32]);
        assert!(own.unwrap_stanza(&damaged).is_none());
        // Another kind of stanza, or one of the wrong shape, is not ours.
        let mut other = copy(&stanzas[0]);
        other.tag = "X25519".into();
        assert!(own.unwrap_stanza(&other).is_none());
        let mut short = copy(&stanzas[0]);
        short.body.pop();
        assert!(own.unwrap_stanza(&short).is_none());
    }

    /// Forward secrecy: with a one-time key, the file key takes the lasting
    /// keys and the one-time ones; once those are gone, nothing opens it.
    #[test]
    fn a_one_time_key_is_needed_beside_the_lasting_ones_and_then_gone() {
        let (x, x_public, secret) = device();
        let once = OnceSecret::generate().unwrap();
        let shown = once.public();
        assert_eq!(shown.to_bytes().len(), ONCE_PUBLIC);
        assert_eq!(
            OncePublic::from_bytes(&shown.to_bytes()).unwrap().id(),
            shown.id()
        );
        assert_eq!(
            OnceSecret::from_bytes(&once.to_bytes())
                .unwrap()
                .public()
                .id(),
            shown.id()
        );
        let key = file_key();
        let to = HybridRecipient::new(x_public, &secret.public())
            .unwrap()
            .with_once(&shown)
            .unwrap();
        let stanza = to.wrap_file_key(&key).unwrap().0.remove(0);
        assert_eq!(stanza.tag, STANZA_TAG_ONCE);
        assert_eq!(stanza.body.len(), 2 * KEM_CIPHERTEXT + 32);
        assert_eq!(stanza_once(&stanza), Some(shown.id()));

        let mut own = HybridIdentity::new(x, secret.clone());
        // The lasting keys alone (all that is left afterwards): nothing.
        assert!(own.unwrap_stanza(&stanza).is_none());
        // Another one-time key: nothing.
        own.once.push(OnceSecret::generate().unwrap());
        assert!(own.unwrap_stanza(&stanza).is_none());
        // The one it was made for: the file key.
        own.once.push(once.clone());
        let opened = own.unwrap_stanza(&stanza).unwrap().unwrap();
        assert_eq!(opened.expose_secret(), key.expose_secret());
        // The one-time key with another device's lasting keys: nothing
        // (whoever copied the key off the screen gains nothing by it).
        let (other_x, _, other_secret) = device();
        let mut thief = HybridIdentity::new(other_x, other_secret);
        thief.once.push(once);
        assert!(thief.unwrap_stanza(&stanza).is_none());
        // Changed anywhere: not opened.
        for at in [0, KEM_CIPHERTEXT, 2 * KEM_CIPHERTEXT + 5] {
            let mut damaged = copy(&stanza);
            damaged.body[at] ^= 1;
            assert!(own.unwrap_stanza(&damaged).is_none(), "{at}");
        }
    }

    #[test]
    fn two_wrappings_of_one_key_share_nothing() {
        let (_, x_public, secret) = device();
        let key = file_key();
        let to = HybridRecipient::new(x_public, &secret.public()).unwrap();
        let (a, b) = (
            to.wrap_file_key(&key).unwrap().0,
            to.wrap_file_key(&key).unwrap().0,
        );
        assert_ne!(a[0].args, b[0].args);
        assert_ne!(a[0].body, b[0].body);
    }

    #[test]
    fn signatures_hold_for_the_message_and_the_key_only() {
        let secret = PqSecret::generate().unwrap();
        let public = secret.public();
        let signature = secret.sign(b"the message");
        assert!(public.verify(b"the message", &signature));
        assert!(!public.verify(b"the massage", &signature));
        assert!(
            !PqSecret::generate()
                .unwrap()
                .public()
                .verify(b"the message", &signature)
        );
        let mut damaged = signature.clone();
        damaged[100] ^= 1;
        assert!(!public.verify(b"the message", &damaged));
        assert!(!public.verify(b"the message", &signature[..SIGNATURE - 1]));
        assert!(!public.verify(b"the message", &[]));
    }

    /// The keys follow from the seeds alone: what is stored is the seeds.
    #[test]
    fn the_same_seeds_are_the_same_keys() {
        let secret = PqSecret {
            kem_seed: [3; KEM_SEED],
            sig_seed: [4; SIG_SEED],
        };
        assert_eq!(secret.public(), secret.clone().public());
        assert_eq!(secret.sign(b"m"), secret.sign(b"m"));
        // Fixed for good: a change here would lock every device out of
        // what was encrypted to it before.
        let digest = blake3::hash(&secret.public().to_bytes());
        assert_eq!(
            digest.to_hex().as_str(),
            "f272cbcd4ad66befced50affeb01062ba45abb59deb43020831f7b5072fe795a",
        );
    }

    /// Public keys come from others (a code, a file): anything may be in them.
    #[test]
    fn keys_that_are_not_keys_are_refused() {
        let public = PqSecret::generate().unwrap().public();
        let bytes = public.to_bytes();
        assert!(PqPublic::from_bytes(&bytes[..bytes.len() - 1]).is_none());
        assert!(PqPublic::from_bytes(&[]).is_none());
        // Numbers not reduced (all bits set): no encapsulation key.
        let mut bad = bytes.clone();
        bad[..2].fill(0xff);
        assert!(PqPublic::from_bytes(&bad).is_none());
        // A verifying key of nonsense verifies nothing, and nothing panics.
        let nonsense = PqPublic {
            kem: public.kem.clone(),
            sig: vec![0xff; SIG_PUBLIC],
        };
        assert!(!nonsense.verify(b"m", &vec![0u8; SIGNATURE]));
    }
}
