//! QRSend protocol core: framing, RaptorQ coding, manifests and payloads.
//!
//! This crate performs no I/O of its own beyond `std::io` traits so it can be
//! shared by the native CLI and the WebAssembly build. The wire format is
//! specified in `docs/PROTOCOL.md`.
//!
//! Features: `crypto` (default) for device identities, encryption and
//! signatures, `link` (part of `crypto`) for the codes that offer a network
//! connection. Without them, transfers go unencrypted only.
//!
//! QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and
//! in other countries.

pub mod base45;
pub mod compress;
#[cfg(feature = "crypto")]
pub mod crypto;
pub mod direct;
pub mod fec;
pub mod feedback;
pub mod frame;
#[cfg(feature = "link")]
pub mod link;
#[cfg(feature = "link")]
pub mod linkcert;
pub mod manifest;
pub mod payload;
#[cfg(feature = "crypto")]
pub mod pq;
pub mod qr;
pub mod receiver;
pub mod resume;
pub mod sanitize;
pub mod schedule;
pub mod sender;
pub mod sound;
pub mod tune;

#[cfg(test)]
mod tests;
