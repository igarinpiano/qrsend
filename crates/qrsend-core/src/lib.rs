//! QRSend protocol core: framing, RaptorQ coding, manifests and payloads.
//!
//! This crate performs no I/O of its own beyond `std::io` traits so it can be
//! shared by the native CLI and the WebAssembly build. The wire format is
//! specified in `docs/PROTOCOL.md`.

pub mod base45;
pub mod compress;
pub mod crypto;
pub mod direct;
pub mod fec;
pub mod feedback;
pub mod frame;
pub mod link;
pub mod manifest;
pub mod payload;
pub mod qr;
pub mod receiver;
pub mod resume;
pub mod sanitize;
pub mod schedule;
pub mod sender;
pub mod sound;

#[cfg(test)]
mod tests;
