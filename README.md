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
  paths are sanitized and decompression is bounded.
- **CLI and browser.** A single Rust binary, plus a web app (work in progress)
  sharing the same Rust core through WebAssembly.

> Status: early development (v0.1). The wire format is specified in
> [docs/PROTOCOL.md](docs/PROTOCOL.md) and may still change.

**Web app:** <https://igarinpiano.github.io/qrsend/>

## Install

```bash
npm install -g qrsend-cli  # prebuilt binaries for macOS, Linux (glibc/musl), Windows
cargo install qrsend       # from source
```

The npm package is named `qrsend-cli`; the command it installs is `qrsend`.
On 64-bit Windows, use the release archive or Cargo for now — those npm
binaries are not published yet.

Or download an archive from [Releases](https://github.com/igarinpiano/qrsend/releases)
(checksums and build provenance attestations included). The Linux musl builds
are static and have no window display (`--display terminal`).

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

Useful options: `--fps 12`, `--display terminal`, and

- `--density auto|low|normal|high|max` — the default `auto` uses small,
  easy-to-scan codes for small transfers;
- `--grid 3`, `--grid 8x4` or `--grid auto` — several codes at once (any
  rectangle up to 64×64; `auto` fills the window).

Receive with a webcam, from a recording of the sender's screen, or from image files:

```bash
qrsend recv --camera -o ~/Downloads                # live, through ffmpeg
qrsend recv --camera --camera-size 1280x720        # ask the camera for larger pictures
qrsend recv --video recording.mp4 -o ~/Downloads   # needs ffmpeg for non-.y4m videos
qrsend recv --images frames/
```

A webcam often hands out small pictures unless asked (640×480 is a common
default on Linux), which is too little for dense codes: `recv` prints the size
it gets, and `--camera-size` / `--camera-fps` ask for more. The web app's
*Color codes* are noticed and read without being asked for.

If something is still missing, `recv` prints a resume code. Run on the sender:

```bash
qrsend send --resume QSR1-XXXXXXXX
```

and receive again — progress is kept in the inbox (`qrsend inbox list`).
After a text arrives you can copy it to the clipboard or save it as a file
(also `qrsend recv --copy`, `qrsend inbox export <id> --copy`).

### As a video

Write the stream to a video instead of showing it — to play on any screen, TV
or projector and record, or to move data through a screen capture:

```bash
qrsend send big.iso --dense --export-video stream.mp4   # needs ffmpeg; .y4m needs nothing
qrsend recv --video stream.mp4 -o out/                   # the file, or a recording of it
```

`--dense` fills every frame with as many codes as fit, choosing the code size
that carries the most data (`--size 3840x2160`, `--scale` pixels per module,
`--fps`, `--passes` for extra repair codes). A 1080p frame at 2 px per module
holds about 40 KB — over 1 MB/s at 30 fps when captured without loss. Reading
finds one code, then walks the grid from it, so frames with hundreds of codes
decode too. `--export-frames DIR` writes PNG frames instead.

### Over anything that carries bytes

Frames do not have to be pictures. `--export-text` writes them as lines of
text and `recv --text` reads them, so the same resumable, verified transfer
works over a serial line, a TCP connection, ssh or a file — and survives lost
or garbled lines.

```bash
qrsend send big.iso --plain --export-text - | nc 192.168.1.20 9000   # sender
nc -l 9000 | qrsend recv --text - -o ~/Downloads                     # receiver
qrsend send notes/ --plain --export-text /dev/ttyUSB0                # serial
```

### Over the local network, started by the codes

With `--lan`, the stream also carries a small code that tells a receiving
`qrsend` where the sender listens. The receiver connects by itself and the
transfer travels through the connection as well: hundreds of megabytes per
second on one machine, whatever the network gives between two. The screen only
has to be read long enough for that one code; it keeps going, and is all there
is if the connection never comes up.

```bash
qrsend send big.iso --plain --lan          # sender: shows codes, listens
qrsend recv --camera -o ~/Downloads        # receiver: reads the offer, connects
```

No address to type and nothing to pair: the code carries a key, everything on
the connection is encrypted with it, and only someone who saw the sender's
screen can connect. The sender stops by itself once the receiver reports that
everything arrived. (`recv --no-lan` keeps to the codes. On macOS the firewall
may ask whether `qrsend` may accept incoming connections.)

A receiving **web app** connects as well: `--lan` also offers the kind of
connection a browser can make (WebRTC), and the browser takes it up by itself
once its camera has read the code. Nothing has to be shown back to the sender.
The other way round works the same: when the web app sends with *Local network
boost* on, `qrsend recv` reads its offer and connects to the browser.

## Web app

The same protocol runs in the browser (Rust core compiled to WebAssembly):
send text, files and folders, receive with the camera or from a video file,
pair devices and keep unfinished transfers in an inbox. It works offline once
loaded (PWA).

- Data is streamed to and from the browser's private file storage (OPFS) in a
  background worker, so large transfers do not have to fit in memory; saving
  as files, to a folder or as a ZIP reads straight from disk.
- The device's private keys are WebCrypto keys that cannot be exported — not
  even by the page itself.
- Up to 8×8 codes at once, or as many as fit the screen.
- A transfer that stopped halfway continues when the sender simply sends the
  same data again (unencrypted transfers): it is the same transfer, bit for
  bit, and the receiver keeps what it had — also of the part it was in the
  middle of. To send only what is missing without any channel back, type the
  resume code the receiver shows into the sender (web or command line).
- “Copy log” at the bottom of every page copies a diagnostic log for bug
  reports: what happened when (steps, timings, counts, connection states) —
  never file names, contents, device IDs or network addresses.
- **Feature preview** (footer link): new ways of sending that are still being
  worked on. Each is off until the sender turns it on; otherwise everything
  works as before.
  - *Automatic speed*: with two-way transfer, the sender finds the fastest
    setting by itself. It tries more pictures per second or more codes per
    picture, sees in the receiver's feedback whether more codes are read per
    second, keeps what helps and takes back what does not — and keeps
    adjusting as conditions change.
  - *Camera guidance*: the receiver says how to hold the camera when
    something is off — closer, further back, hold still, or fewer codes.
  - *Feedback by sound*: the receiver answers with short runs of soft notes
    from its speaker, heard by the sender's microphone — two-way transfer
    without a camera that sees the receiver's screen. The receiver is asked
    every time before it makes a sound.
  - *Local network boost*: once each device has seen the other's screen, they
    also connect directly over the local network (no server, and nothing
    outside the network is contacted). Both ways are
    then used at once — megabytes per second instead of kilobytes — and the
    screen carries on alone if the connection drops. The sender finds the
    pace the connection can take as it goes.
  - *Remember trusted devices*: a device you trust that has connected over
    the local network once is connected to directly from then on — its
    screen no longer has to be shown to the sender's camera.
  - *Color codes*: three codes in one, as the red, green and blue parts of
    the picture — up to three times the data per frame. Receivers recognize
    them on their own.
  - *Receive from the screen*: read the codes straight from a window or
    screen (a remote desktop, a virtual machine, a shared screen in a call)
    instead of through a camera.
  - *Two-way transfer*: when the sender has a camera that sees the receiver's
    screen (two phones or laptops facing each other), the receiver shows a
    small feedback code with what is still missing. The sender sends only
    that — lost codes are made up for at once instead of a whole pass later —
    and stops by itself when everything has arrived. The receiver needs no
    setting, and if the feedback drops out, sending simply carries on the
    usual way.

```bash
cd web && npm ci && npm run wasm && npm run dev
```

It is hosted at <https://igarinpiano.github.io/qrsend/>, and each release also
ships an offline copy (`qrsend-web-*.zip`).

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
- [x] Live camera capture in the CLI (via ffmpeg)
- [x] Device identities, public-key encryption (age) and sender signatures
- [x] Web app (PWA) with camera receive, pairing and inbox
- [x] Streaming storage (OPFS) and non-extractable keys in the browser
- [x] Dense grids and video export / import
- [x] A back channel from receiver to sender: acknowledgments (web app, feature preview)
- [x] Auto-tuning speed and codes per picture from that feedback (web app, feature preview)
- [x] More transports side by side: text over any byte channel (CLI), color codes, screen capture, local network (web app, feature preview)
- [x] Sound as a back channel (web app, feature preview)
- [x] A direct network connection between two CLIs, started by a code (`send --lan`), and between the CLI and a browser in either direction
- [ ] Two-way mode and sound in the CLI; auto-tuning the code size

Design notes (Japanese): [docs/CONCEPT.md](docs/CONCEPT.md).

## License

Apache-2.0

QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
