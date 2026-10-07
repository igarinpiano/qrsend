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
 * Colour codes stack three codes as the red, green and blue parts of the
 * picture. Returns what each part holds, read as a grey picture of its own.
 */
async function detectColours(bitmap: ImageBitmap, native: BarcodeDetector | null): Promise<string[][]> {
  const rgba = pixels(bitmap).data;
  const out: string[][] = [];
  for (let c = 0; c < 3; c++) {
    const grey = new ImageData(bitmap.width, bitmap.height);
    const d = grey.data;
    for (let i = 0; i < rgba.length; i += 4) {
      const v = rgba[i + c];
      d[i] = v;
      d[i + 1] = v;
      d[i + 2] = v;
      d[i + 3] = 255;
    }
    out.push(await detect(grey, native));
  }
  return out;
}

// Nothing tells a receiver that the codes are coloured. Seen as a grey
// picture, a coloured frame still yields a code now and then (mostly the
// green one), so every so often the three parts are read separately: if they
// hold different codes, the stream is coloured and is read that way from then
// on; if they all hold the same, it is black and white.
const COLOUR_PROBE_EVERY = 12;
let coloured = false;
let sinceProbe = 0;

async function scan(bitmap: ImageBitmap, native: BarcodeDetector | null): Promise<string[]> {
  sinceProbe++;
  if (!coloured && sinceProbe < COLOUR_PROBE_EVERY) return detect(bitmap, native);
  const parts = await detectColours(bitmap, native);
  const all = [...new Set(parts.flat())];
  const most = Math.max(...parts.map((p) => p.length));
  if (all.length > most) {
    coloured = true;
  } else if (sinceProbe >= COLOUR_PROBE_EVERY) {
    // Three times the same (or nothing at all): not coloured.
    coloured = false;
  }
  if (sinceProbe >= COLOUR_PROBE_EVERY) sinceProbe = 0;
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
    texts = await scan(bitmap, native);
    self.postMessage({ texts, engine, coloured });
  } catch (err) {
    self.postMessage({ texts: [], engine, coloured, error: String(err) });
  } finally {
    bitmap.close();
  }
};
