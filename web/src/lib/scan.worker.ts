/// <reference lib="webworker" />
// Decodes QR codes from camera frames off the main thread: the platform's
// BarcodeDetector when it supports QR, otherwise ZXing (WebAssembly).
import { prepareZXingModule, readBarcodes } from "zxing-wasm/reader";
import zxingWasmUrl from "zxing-wasm/reader/zxing_reader.wasm?url";

declare class BarcodeDetector {
  constructor(options: { formats: string[] });
  static getSupportedFormats(): Promise<string[]>;
  detect(source: ImageBitmapSource): Promise<{ rawValue: string }[]>;
}

prepareZXingModule({
  overrides: {
    locateFile: (path: string, prefix: string) => (path.endsWith(".wasm") ? zxingWasmUrl : prefix + path),
  },
});

let detector: BarcodeDetector | null | undefined;
let forceZxing = false;

async function nativeDetector(): Promise<BarcodeDetector | null> {
  if (detector !== undefined) return detector;
  detector = null;
  try {
    if (!forceZxing && "BarcodeDetector" in self) {
      const formats = await BarcodeDetector.getSupportedFormats();
      if (formats.includes("qr_code")) detector = new BarcodeDetector({ formats: ["qr_code"] });
    }
  } catch {
    detector = null;
  }
  return detector;
}

let canvas: OffscreenCanvas | undefined;

function pixels(bitmap: ImageBitmap): ImageData {
  if (!canvas || canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
    canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  }
  const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
  ctx.drawImage(bitmap, 0, 0);
  return ctx.getImageData(0, 0, bitmap.width, bitmap.height);
}

async function detect(source: ImageBitmap | ImageData, native: BarcodeDetector | null): Promise<string[]> {
  if (native) return (await native.detect(source)).map((b) => b.rawValue);
  const image = source instanceof ImageData ? source : pixels(source);
  // Grids can hold many codes; ask for all of them.
  const results = await readBarcodes(image, { formats: ["QRCode"], maxNumberOfSymbols: 255, tryHarder: true });
  return results.filter((r) => r.isValid).map((r) => r.text);
}

/**
 * Color codes stack three codes as the red, green and blue parts of the
 * picture. Returns what each part holds, read as a gray picture of its own.
 */
async function detectColors(bitmap: ImageBitmap, native: BarcodeDetector | null): Promise<string[][]> {
  const rgba = pixels(bitmap).data;
  const out: string[][] = [];
  for (let c = 0; c < 3; c++) {
    const gray = new ImageData(bitmap.width, bitmap.height);
    const d = gray.data;
    for (let i = 0; i < rgba.length; i += 4) {
      const v = rgba[i + c];
      d[i] = v;
      d[i + 1] = v;
      d[i + 2] = v;
      d[i + 3] = 255;
    }
    out.push(await detect(gray, native));
  }
  return out;
}

// Nothing tells a receiver that the codes are colored. Seen as a gray
// picture, a colored frame still yields a code now and then (mostly the
// green one), so every so often the three parts are read separately. The
// answer changes only on evidence: parts holding different codes mean
// colored; at least two parts holding exactly the same codes mean black and
// white. A picture in which nothing, or only one part, could be read says
// nothing either way (a weak camera loses the red and blue parts often), and
// the answer stays what it was.
const COLOR_PROBE_EVERY = 12;
let colored = false;
let sinceProbe = 0;

async function scan(bitmap: ImageBitmap, native: BarcodeDetector | null): Promise<string[]> {
  sinceProbe++;
  if (!colored && sinceProbe < COLOR_PROBE_EVERY) return detect(bitmap, native);
  sinceProbe = 0;
  const parts = await detectColors(bitmap, native);
  const all = [...new Set(parts.flat())];
  const read = parts.filter((p) => p.length > 0);
  const most = Math.max(...parts.map((p) => p.length));
  if (all.length > most) {
    colored = true;
  } else if (read.length >= 2) {
    // Every part that could be read holds the same codes.
    colored = false;
  }
  return all;
}

self.onmessage = async (e: MessageEvent<{ bitmap?: ImageBitmap; zxing?: boolean }>) => {
  if (e.data.zxing !== undefined) {
    forceZxing = e.data.zxing;
    detector = undefined;
    return;
  }
  const bitmap = e.data.bitmap!;
  let texts: string[] = [];
  let engine = "zxing";
  try {
    const native = await nativeDetector();
    if (native) engine = "native";
    const started = performance.now();
    texts = await scan(bitmap, native);
    self.postMessage({ texts, engine, colored, ms: performance.now() - started });
  } catch (err) {
    self.postMessage({ texts: [], engine, colored, error: String(err) });
  } finally {
    bitmap.close();
  }
};
