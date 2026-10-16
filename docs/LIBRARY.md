# The library for web pages (`packages/qrsend-web`)

An npm package that lets other people's web pages send and receive QRSend
transfers. How to use it is in [packages/qrsend-web/README.md](../packages/qrsend-web/README.md);
this document records what was decided and how it is built. Like everything
of the library, it is in English (the other documents of the repository are
in Japanese).

## Decisions (2026-10-16, with the project's owner)

- **Scope**: turning the data to send into QR codes, showing them, and reading
  them back into the data with a camera. **No encryption, no device IDs, no
  trusted devices**: unencrypted transfers only.
- **What it carries**: texts, and named files in folders.
- **Ready-made parts too**: besides encoding and decoding, a player that shows
  the codes (`QrPlayer`) and a scanner that reads them with a camera
  (`QrScanner`).
- **The player can be paused and resumed, stepped forward and back picture by
  picture, and its speed set as milliseconds per picture** (`interval`; not
  "faster / slower" and not pictures per second).
- **The package name is chosen at publishing.** For now it is `@qrsend/web`,
  and `package.json` says `"private": true` so that it cannot be published by
  mistake.
- **Trademark**: the README, the comments at the top of the sources and the
  constant `QR_CODE_TRADEMARK` ask pages that use the library to carry the QR
  Code trademark notice. The words on the page are the user's to write.
- **Everything of the library is in English**: code, comments, README, this
  document, the examples and the tests.

## Layout

```
crates/qrsend-core/   encryption can be left out with features (below)
crates/qrsend-js/     the library's WebAssembly (wasm-bindgen; not published): Encoder, Decoder
packages/qrsend-web/  the npm package
  src/index.ts        init, Encoder, Decoder, QR_CODE_TRADEMARK (also in Node)
  src/player.ts       QrPlayer (draws on a canvas), drawCodes
  src/scanner.ts      QrScanner (reads a camera or a MediaStream), readCodes
  src/detect.ts       BarcodeDetector, else ZXing (zxing-wasm)
  src/scan-worker.ts  finds the codes on a worker
  scripts/build.mjs   the WebAssembly, wasm-bindgen, then tsc, into dist/
  test/               Node tests (round trips, both ways against the CLI)
  example/            example pages (send, receive, a page that sends to itself), built with Vite for the e2e tests
  e2e/                Playwright
```

### Features of the core

- `crypto` (on by default): the `crypto` and `pq` modules (age, ed25519,
  x25519, ML-KEM, ML-DSA, sha3, getrandom, base64, bech32). Includes `link`.
- `link`: the `link` and `linkcert` modules (p256, sha2).
- Without either, the core handles unencrypted transfers only. It still reads
  meta envelopes that are signed, scheme 2 included (the length of an ML-DSA
  signature moved to `manifest::PQ_SIGNATURE`; `pq::SIGNATURE` refers to it),
  without checking the signature.
- The CLI and the fuzz targets use the defaults; `qrsend-wasm` (the web app)
  names `features = ["ruzstd", "crypto"]`.
- Size: the library's WebAssembly is 875 kB (320 kB gzipped); the app's is
  2.8 MB (before wasm-bindgen).

### Sending (`Encoder`)

- Everything is held in WebAssembly memory (`MemorySource`). That suits what
  QR codes carry (tens of kilobytes a second); for hundreds of megabytes the
  README points to the app and the CLI.
- The session id follows from the content (`content_session_id`, as for the
  app's unencrypted transfers): sending the same data again lets a receiver
  that stopped halfway continue.
- The code size is `qr::auto_params` by default (small codes for small
  transfers, version 25-L for the rest). With a version given, the error
  correction defaults to M.
- **Paths a receiver would refuse (`..`, absolute paths) are refused before
  sending** (`SafePath::parse`). At first they were taken, and unpacking
  failed on the receiving side with "unsafe path" (found by a test). Names
  that differ only in case, and names Windows refuses, are renamed by the
  receiver as before.
- Compression is ruzstd, as in the app (about zstd level 1).
- `render(text)` draws a code's text again, at the encoder's version: the
  player keeps the texts of the pictures it showed (about 1.6 kB a code)
  instead of their modules (about 14 kB a code at version 25).

### Receiving (`Decoder`)

- It follows the first transfer it reads codes of, and passes over the codes
  of others (`otherTransfer`), texts that are not QRSend codes, notices
  (QSC1) and link codes (QSL1).
- **Codes of an encrypted transfer are passed over before it locks on to a
  transfer** (and `encrypted` is set). Locking on to an encrypted transfer
  would keep it from taking an unencrypted one shown afterwards.
- Checks as in the app: every segment's BLAKE3 (those that came before the
  file list, once it arrives), the whole body's BLAKE3, every file's BLAKE3.
  `payload::unpack` makes the paths safe.
- `push` returns whether `progress` changed, so that `progress` is not turned
  into JSON for every code.

### Showing (`QrPlayer`)

- One picture every `interval` milliseconds, on requestAnimationFrame (so no
  faster than the display). The rate holds on average, without catching up
  after a pause.
- `grid`: 1 (default), a number, `[columns, rows]` or `"fill"`. The 4 modules
  of white are shared with the neighbors (as `qr::QUIET`). A module is a whole
  number of device pixels.
- Pausing, stepping: the player keeps the last `history` pictures (default
  300) as the texts of their codes, with their columns and rows.
  `stepBack()` shows them again as they were; going on from there (playing or
  `stepForward()`) goes through them before new pictures are made. Stepping
  pauses. A canvas that changes size while paused gets the same picture drawn
  anew.
- Not here: the app's color codes, automatic speed, two-way transfer and
  network connections.

### Reading (`QrScanner`)

- The codes are found with the BarcodeDetector (where it reads QR codes),
  otherwise with ZXing. Without `zxingWasmUrl`, ZXing's WebAssembly comes
  from where zxing-wasm takes it by default (the jsDelivr CDN); devices with
  a BarcodeDetector never fetch it.
- On a worker by default (`new URL("./scan-worker.js", import.meta.url)`).
  When the worker cannot be made or loaded, the codes are found on the page
  (as with `worker: false`).
- The next picture is taken once the last one is read (requestVideoFrameCallback,
  else requestAnimationFrame).
- `receive()` closes the camera when the transfer is complete and resolves
  with it. `stop()` rejects it with an AbortError; another `receive()` goes on
  where it was (`reset()` starts over).

## What was checked

- Rust unit tests (`crates/qrsend-js`: a transfer comes together with a third
  of its codes lost and the rest back to front; the same data makes the same
  transfer; codes have the size asked for).
  `cargo test -p qrsend-core --no-default-features --features ruzstd` (the
  core without encryption).
- Node (`npm test`): round trips of texts, files, folders, Blobs and bytes;
  lost and reordered codes; paths; the same data makes the same transfer;
  other transfers and unrelated texts are passed over; progress never goes
  down; `render` draws a text as `nextModules` does. **Against the CLI**: the
  library reads the CLI's `send --plain --export-text`, the CLI's
  `recv --text` reads the library's codes, and the CLI's encrypted transfer
  is passed over as `encrypted`.
- Browser (`npm run test:e2e`, Chromium): the examples built with Vite. The
  player's canvas goes to the scanner through `captureStream` (with and
  without the worker, and four codes a picture). Chrome's fake camera plays
  the CLI's `--export-y4m`, and the receive page takes the transfer. The send
  page shows the trademark notice. The controls: paused, the picture stays;
  forward gives a new picture, back the one before exactly as it was, forward
  again replays, then new ones; space plays; 500 ms per picture gives about 2
  pictures a second and 50 ms about 20.
- Installed from npm: the packed package (`npm pack`) in a separate Vite
  project, without any configuration, worked in the dev server and in a
  production build (the WebAssembly and the worker end up in the output).
- **Not checked**: real cameras (above all Safari on the iPhone, which uses
  ZXing), the BarcodeDetector path (Chromium on Linux has none), bundlers
  other than Vite, no bundler (an import map), the web app as the other side
  (the same core, so it should work; the tests pair the library with the CLI
  only).

## To do at publishing (not done yet)

1. Choose the name: replace it in `package.json` (`name`), the README,
   `example/vite.config.js` (the aliases) and the comments of the sources
   (`@qrsend/web/player`, …). Under the npm organization `qrsend`, a new
   name is less likely to be held up by the spam check.
2. Remove `"private": true` from `package.json`.
3. Add an npm step to `publish-all.yml` (`npm run build` in
   `packages/qrsend-web`, then `npm publish --provenance`). One package, no
   per-platform ones.
4. As a new package, publish the first version by hand and register its
   Trusted Publisher (as for the CLI's per-platform packages).
5. Keep the version with the workspace's (`packages/qrsend-web/package.json`
   is in the list of files to raise together).

---

QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
