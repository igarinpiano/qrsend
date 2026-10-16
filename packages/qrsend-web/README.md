# @qrsend/web

**Send files, folders and text from one screen to another device's camera, as
a stream of QR codes.** No network, no server, no pairing: one page shows the
codes, the other reads them with its camera and gets the files back.

- **Any order, any gaps.** The codes are fountain-coded (RaptorQ): the
  receiver can start at any point and miss any of them; enough of them
  rebuild the data.
- **Checked.** Every part and every file is checked against BLAKE3 hashes
  before you get it, and paths are made safe to save.
- **Compatible.** The stream is the one of the [QRSend](https://github.com/igarinpiano/qrsend)
  app and CLI, so either side can be them (unencrypted transfers).
- **Small API.** `QrPlayer` shows, `QrScanner` reads; `Encoder` and `Decoder`
  underneath, for codes you show or read some other way. The encoding and
  decoding also run in Node.

## Install

```bash
npm install @qrsend/web
```

The package is ES modules with TypeScript types. Its core is WebAssembly
(about 320 kB gzipped), loaded with `new URL(…, import.meta.url)`, as is the
scanner's worker: Vite, webpack 5 and other bundlers that understand that
pattern pick both up without any configuration (tested with Vite).

## Send

```html
<canvas id="codes" style="width: 480px; height: 480px"></canvas>
<input type="file" id="files" multiple />
<script type="module">
  import { QrPlayer } from "@qrsend/web/player";

  const player = new QrPlayer(document.getElementById("codes"), { fps: 10 });
  document.getElementById("files").onchange = (e) => player.play(e.target.files);
</script>
```

`play()` takes:

| Content | Sent as |
|---|---|
| a string | a text |
| a `File` | one file with its name |
| a `Blob`, `ArrayBuffer` or typed array | one file named `options.name` (default `data`) |
| a `FileList` or an array of `File`s | several files, in their folders when they come from `<input webkitdirectory>` |
| an array of `{ path, data?, lastModified? }` | files (and, without `data`, folders) at the paths you give |

The player goes on until `stop()`. Give the canvas its size with CSS; the
codes are drawn sharp at the device's pixel ratio. Options:

| Option | Default | |
|---|---|---|
| `fps` | `10` | Pictures per second. Slower is easier for a weak camera. Can be changed while playing. |
| `grid` | `1` | Codes per picture: a number, `[columns, rows]`, or `"fill"` (as many as fit). More codes send more, but each comes out smaller. |
| `minModulePixels` | `4` | With `grid: "fill"`: the smallest module, in device pixels. |
| `qrVersion` | chosen | QR version 1–40. Left out, small transfers get small codes and the rest version 25. |
| `errorCorrection` | `"M"` | With `qrVersion`: `"L"`, `"M"`, `"Q"` or `"H"`. |
| `compress` | `true` | zstd. Turn off for data that is compressed already (photos, videos, archives). |
| `redundancy` | `0.1` | Extra codes in each round, as a fraction of what the data takes. |

The same content always makes the same transfer: a receiver that stopped
halfway continues where it was when you send it again.

## Receive

```html
<video id="camera"></video>
<progress id="bar" max="1"></progress>
<script type="module">
  import { QrScanner } from "@qrsend/web/scanner";

  const scanner = new QrScanner(document.getElementById("camera"));
  scanner.onprogress = (p) => (document.getElementById("bar").value = p.ratio);

  const received = await scanner.receive();
  if (received.kind === "text") {
    console.log(received.text);
  } else {
    for (const file of received.files) {
      // file.path ("photos/a.jpg"), file.name, file.data (a Blob), file.lastModified
    }
  }
</script>
```

`receive()` opens the camera, reads until everything is there and checked,
closes the camera and resolves. `stop()` makes it reject with an
`AbortError`; a later `receive()` goes on where it was (`reset()` starts
over).

The browser lets a page use the camera only over HTTPS (or from
`localhost`), and asks the user first.

Options of `QrScanner`:

| Option | |
|---|---|
| `deviceId` | The camera to use (`QrScanner.cameras()` lists them). |
| `facingMode` | Without `deviceId`: `"environment"` (default, the back camera) or `"user"`. |
| `stream` | Read this `MediaStream` instead of a camera: a shared screen (`getDisplayMedia()`), a `canvas.captureStream()`, … |
| `worker` | Find the codes on a worker, off the page's main thread (default `true`). |
| `zxingWasmUrl` | Where ZXing's WebAssembly is served from (see below). |

`progress` (also `scanner.decoder.progress`) tells `state` (`"waiting"`,
`"receiving"`, `"complete"`), `ratio` (0 to 1), `receivedBytes` /
`totalBytes`, `usefulCodes`, `remainingCodes`, and once the list of files has
arrived `kind`, `fileCount` and `size`; `scanner.decoder.entries()` lists the
files before they arrive. `encrypted` is set when codes of an encrypted
QRSend transfer were read: those cannot be opened with this library and are
passed over.

### Finding the codes

Where the browser has a `BarcodeDetector` for QR codes (Chrome on Android
and macOS), the scanner uses it. Elsewhere (Safari, iPhone and iPad, Firefox,
Chrome on Windows and Linux) it uses [ZXing](https://github.com/Sec-ant/zxing-wasm),
whose WebAssembly (about 1 MB) is fetched from the jsDelivr CDN unless you
serve it yourself:

```js
// With Vite:
import zxingWasmUrl from "zxing-wasm/reader/zxing_reader.wasm?url";
const scanner = new QrScanner(video, { zxingWasmUrl });
```

## Lower level: `Encoder` and `Decoder`

For codes you draw or read some other way, from `@qrsend/web` (these also
run in Node):

```js
import { Decoder, Encoder } from "@qrsend/web";

const encoder = await Encoder.create(files);
encoder.next();         // the text of the next code (QR alphanumeric mode, version `encoder.qrVersion`)
encoder.nextModules();  // or the code drawn: encoder.size × encoder.size, 1 = dark
encoder.codesPerRound;  // codes that hold everything once

const decoder = await Decoder.create();
decoder.push(text);     // the text of any code read; returns whether progress changed
if (decoder.complete) decoder.result();  // { kind: "text", text } or { kind: "files", files, directories }
```

`readCodes(picture)` from `@qrsend/web/scanner` finds the codes in one
picture (an image, a canvas, a `Blob`). `init(source)` loads the
WebAssembly from somewhere else than next to the module (its URL or bytes);
everything that needs it calls `init()` itself.

Codes are made in QR alphanumeric mode at a fixed version: to draw them
with a QR library of your own, make it use that mode and version, or use
`nextModules()`.

## Things to know

- **Unencrypted only.** Anyone who can see the codes can read the data. For
  encryption to paired devices, use the QRSend app or CLI.
- **Memory.** A transfer is held in memory while it is sent or put together
  (as compressed, plus the files once received). That suits what QR codes
  carry, tens of kilobytes a second; for hundreds of megabytes, use the QRSend
  app or CLI.
- **Content-Security-Policy.** WebAssembly needs `script-src 'wasm-unsafe-eval'`,
  the scanner's worker `worker-src 'self'` (without it, the codes are found
  on the page instead), and ZXing from the CDN `connect-src https://fastly.jsdelivr.net`.
- **Without a bundler**, map `zxing-wasm` with an import map (the package
  imports `zxing-wasm/reader`).

## Trademark

"QR Code" is a registered trademark of DENSO WAVE INCORPORATED. A page or app
that shows QR codes made with this library, or names them, should carry the
notice where people see it: a footer, an about page, the help. The sentence
is exported as `QR_CODE_TRADEMARK`:

```js
import { QR_CODE_TRADEMARK } from "@qrsend/web";
document.querySelector("footer").textContent = QR_CODE_TRADEMARK;
```

```
QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
```

## License

Apache-2.0

QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.
