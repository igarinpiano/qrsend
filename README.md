# QRSend

**Send any data — text, files, folders — through a stream of QR codes.**
No network, no accounts, no pairing server: one screen shows an endless,
fountain-coded stream of QR codes, and a camera (or a video of the screen)
rebuilds the data from any large-enough subset of them.

- **Any size.** Data is split into segments and RaptorQ-coded; transfers can
  take hours, be interrupted, and be resumed days later.
- **Loss tolerant.** Missed frames don't matter — any ~K+ε symbols rebuild a
  segment. Missing pieces can be requested with a short *resume code*.
- **Private and verified.** End-to-end encryption to paired devices (age) and
  signed manifests; every segment and file is checked against BLAKE3 hashes,
  paths are sanitised and decompression is bounded.
- **CLI and browser.** A single Rust binary, plus a web app (work in progress)
  sharing the same Rust core through WebAssembly.

> Status: early development (v0.1). The wire format is specified in
> [docs/PROTOCOL.md](docs/PROTOCOL.md) and may still change.

## Install

From a [release](https://github.com/igarinpiano/qrsend/releases) archive, or with Cargo:

```bash
cargo install --git https://github.com/igarinpiano/qrsend qrsend
```

## Usage

### Pair devices (once)

Every device has a key pair. On the **receiving** device:

```bash
qrsend id            # prints this device's ID, fingerprint and a QR code
```

On the **sending** device, trust it (paste the ID, or scan a screenshot of the QR):

```bash
qrsend devices add 'qrsend-id:1:age1…'      # or: qrsend devices add --image id.png
```

Check that both screens show the same fingerprint. Do the same in the other
direction so the receiver can verify who sent a transfer (`From: laptop ✓`).

### Send

Transfers are encrypted (age, X25519) for the devices you name and signed
(Ed25519) by the sender. Sending unencrypted requires `--plain`.

```bash
qrsend send photos/ notes.md --to phone
qrsend send --text "hello from the other screen" --to phone --to tablet
tar c project | qrsend send - --name project.tar --plain
```

Useful options: `--density low|normal|high|max`, `--fps 12`, `--grid 2`
(2×2 codes at once), `--display terminal`.

Receive from a recording of the sender's screen, or from image files:

```bash
qrsend recv --video recording.mp4 -o ~/Downloads   # needs ffmpeg for non-.y4m videos
qrsend recv --images frames/
```

If something is still missing, `recv` prints a resume code. Run on the sender:

```bash
qrsend send --resume QSR1-XXXXXXXX
```

and receive again — progress is kept in the inbox (`qrsend inbox list`).

Export instead of displaying (for testing, or to play the stream elsewhere):

```bash
qrsend send big.iso --export-y4m stream.y4m   # also usable as a fake camera in Chrome
qrsend send notes.md --export-frames frames/
```

## How it works

```
files ─ concat ─ zstd ─▶ body ─ split ─▶ segments ─┐
manifest (paths, sizes, BLAKE3) ─▶ meta segment ───┴─ RaptorQ ─▶ symbols ─▶ frames ─ Base45 ─▶ QR
```

Each QR code carries one RaptorQ symbol of one segment plus a 26-byte header.
Frames are Base45-encoded so they fit QR alphanumeric mode and survive
decoders that only return text. See [docs/PROTOCOL.md](docs/PROTOCOL.md).

## Roadmap

- [x] v0.1 — protocol core, CLI send (window / terminal / export) and receive (video / images), inbox and resume codes
- [ ] Live camera capture in the CLI
- [x] Device identities, public-key encryption (age) and sender signatures
- [ ] Web app (PWA on GitHub Pages) with camera receive and OPFS storage
- [ ] Higher-throughput modes (colour codes, two-way auto-tuning)

Design notes (Japanese): [docs/CONCEPT.md](docs/CONCEPT.md).

## License

Apache-2.0
