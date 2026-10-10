//! A certificate that follows from a seed (docs/PROTOCOL.md §12.5).
//!
//! A browser connects (WebRTC) only to a peer whose DTLS certificate it was
//! told the fingerprint of. A receiver that cannot send anything back to the
//! sender, a command-line program read by nobody's camera, cannot tell it. So
//! the sender puts a seed into its offer, the receiver makes its certificate
//! from the seed, and the sender works out the fingerprint of that same
//! certificate by itself. Everything here is therefore deterministic: the
//! key, every field of the certificate, and the signature (RFC 6979).
//!
//! Whoever reads the offer can make this certificate, which is the point:
//! the connection is open to whoever saw the sender's screen, as the codes
//! themselves are.

use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use sha2::{Digest, Sha256};

/// A DTLS certificate (X.509, ECDSA P-256) and its private key (PKCS #8),
/// both in DER.
pub struct LinkCertificate {
    pub certificate: Vec<u8>,
    pub private_key: Vec<u8>,
}

impl LinkCertificate {
    /// What WebRTC knows a certificate by: its SHA-256.
    pub fn fingerprint(&self) -> [u8; 32] {
        Sha256::digest(&self.certificate).into()
    }
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    match content.len() {
        n @ 0..=0x7f => out.push(n as u8),
        n @ 0x80..=0xff => out.extend_from_slice(&[0x81, n as u8]),
        n => out.extend_from_slice(&[0x82, (n >> 8) as u8, n as u8]),
    }
    out.extend_from_slice(content);
    out
}

const SEQUENCE: u8 = 0x30;
const BIT_STRING: u8 = 0x03;
const OCTET_STRING: u8 = 0x04;
/// AlgorithmIdentifier: ecdsa-with-SHA256.
const ECDSA_SHA256: [u8; 12] = [
    0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02,
];
/// AlgorithmIdentifier: id-ecPublicKey, prime256v1.
const EC_P256: [u8; 21] = [
    0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48,
    0xce, 0x3d, 0x03, 0x01, 0x07,
];
/// Name: CN=qrsend.
const NAME: [u8; 19] = [
    0x30, 0x11, 0x31, 0x0f, 0x30, 0x0d, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x06, b'q', b'r', b's',
    b'e', b'n', b'd',
];

fn utc_time(text: &[u8; 13]) -> Vec<u8> {
    der(0x17, text)
}

/// The certificate that follows from `seed`.
pub fn certificate(seed: &[u8]) -> LinkCertificate {
    // (One hash in four billion is no valid key: then the next one.)
    let key = (0u32..)
        .find_map(|n| {
            let mut h = blake3::Hasher::new_derive_key("qrsend link certificate key v1");
            h.update(seed);
            h.update(&n.to_le_bytes());
            SigningKey::from_slice(h.finalize().as_bytes()).ok()
        })
        .expect("some hash is a key");
    let point = key.verifying_key().to_sec1_point(false);
    let mut public = vec![0u8];
    public.extend_from_slice(point.as_bytes());
    let mut serial = *blake3::derive_key("qrsend link certificate serial v1", seed)
        .first_chunk::<8>()
        .unwrap();
    serial[0] = (serial[0] & 0x7f) | 0x40;

    let mut tbs = vec![0xa0, 0x03, 0x02, 0x01, 0x02]; // version 3
    tbs.extend(der(0x02, &serial));
    tbs.extend_from_slice(&ECDSA_SHA256);
    tbs.extend_from_slice(&NAME);
    // Peers go by the fingerprint, not by these dates; they only have to be
    // the same for everyone.
    tbs.extend(der(
        SEQUENCE,
        &[utc_time(b"240101000000Z"), utc_time(b"491231235959Z")].concat(),
    ));
    tbs.extend_from_slice(&NAME);
    tbs.extend(der(
        SEQUENCE,
        &[EC_P256.to_vec(), der(BIT_STRING, &public)].concat(),
    ));
    let tbs = der(SEQUENCE, &tbs);

    let signature: Signature = key.sign(&tbs);
    let mut signed = vec![0u8];
    signed.extend_from_slice(signature.to_der().as_bytes());
    let certificate = der(
        SEQUENCE,
        &[tbs, ECDSA_SHA256.to_vec(), der(BIT_STRING, &signed)].concat(),
    );
    // PKCS #8 around the key as SEC 1 writes it (with its public part).
    let sec1 = der(
        SEQUENCE,
        &[
            vec![0x02, 0x01, 0x01],
            der(OCTET_STRING, &key.to_bytes()),
            der(0xa1, &der(BIT_STRING, &public)),
        ]
        .concat(),
    );
    let private_key = der(
        SEQUENCE,
        &[
            vec![0x02, 0x01, 0x00],
            EC_P256.to_vec(),
            der(OCTET_STRING, &sec1),
        ]
        .concat(),
    );
    LinkCertificate {
        certificate,
        private_key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_makes_the_same_certificate() {
        let a = certificate(&[7; 16]);
        let b = certificate(&[7; 16]);
        assert_eq!(a.certificate, b.certificate);
        assert_eq!(a.private_key, b.private_key);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(certificate(&[8; 16]).fingerprint(), a.fingerprint());
        // An X.509 certificate of the usual size for this kind of key.
        assert_eq!(a.certificate[0], SEQUENCE);
        assert!((250..400).contains(&a.certificate.len()));
    }

    /// The certificate must not change with a library update: a browser and
    /// the command line of different versions work it out on their own and
    /// have to agree (computed with p256 0.13, kept with 0.14).
    #[test]
    fn the_certificate_of_a_seed_never_changes() {
        let c = certificate(&[7; 16]);
        let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(
            hex(&c.fingerprint()),
            "1470c28fa1a18459507e292a48bbb2e7d99b51c42134a0977249f9e603ef1f08"
        );
        assert_eq!(
            hex(&Sha256::digest(&c.private_key)),
            "d44d7c71b04db2c453de4253c2606f2d8dc920ca973140a5cf8d7fe1adad8076"
        );
    }

    #[test]
    fn lengths_in_der() {
        assert_eq!(der(0x04, &[1, 2]), [0x04, 2, 1, 2]);
        assert_eq!(der(0x04, &[0; 200])[..3], [0x04, 0x81, 200]);
        assert_eq!(der(0x04, &[0; 300])[..4], [0x04, 0x82, 1, 44]);
    }
}
