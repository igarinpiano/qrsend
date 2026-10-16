# qrsend-core

Protocol core of [QRSend](https://github.com/igarinpiano/qrsend): send any data
through a stream of QR codes.

- Frame format (Base45, QR alphanumeric mode) with CRC-32
- RaptorQ (RFC 6330) fountain coding of fixed-size segments
- Manifest with BLAKE3 hashes, zstd-compressed payloads
- Device identities, age encryption and signed manifests (X25519 and Ed25519,
  with ML-KEM-768 and ML-DSA-65 alongside)
- Path sanitization, resume codes, QR rendering and detection

No I/O beyond `std::io` traits, so the same code runs natively and in
WebAssembly. The wire format is specified in
[docs/PROTOCOL.md](https://github.com/igarinpiano/qrsend/blob/main/docs/PROTOCOL.md).

Features: `zstd-native` (default, C library), `zstdmt`, `ruzstd` (pure Rust,
for WebAssembly); `crypto` (default) for device identities, encryption and
signatures, and `link` (part of `crypto`) for the codes that offer a network
connection. Without `crypto`, transfers go unencrypted only.

License: Apache-2.0

QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
