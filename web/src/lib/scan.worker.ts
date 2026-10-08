/// <reference lib="webworker" />
// Decodes QR codes from camera frames off the main thread: the platform's
// BarcodeDetector when it supports QR, otherwise ZXing (WebAssembly).
import { prepareZXingModule, readBarcodes } from "zxing-wasm/reader";
import zxingWasmUrl from "zxing-wasm/reader/zxing_reader.wasm?url";
import type { Box } from "./guide";

type Point = { x: number; y: number };

declare class BarcodeDetector {
  constructor(options: { formats: string[] });
  static getSupportedFormats(): Promise<string[]>;
  detect(source: ImageBitmapSource): Promise<{ rawValue: string; cornerPoints?: Point[] }[]>;
}

/** A code's text and where it was found. */
interface Found {
  text: string;
  corners?: Point[];
}

function box(found: Found): Box | undefined {
  const c = found.corners;
  if (!c?.length) return undefined;
  const xs = c.map((p) => p.x);
  const ys = c.map((p) => p.y);
  return { x0: Math.min(...xs), y0: Math.min(...ys), x1: Math.max(...xs), y1: Math.max(...ys), chars: found.text.length };
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

async function detect(source: ImageBitmap | ImageData, native: BarcodeDetector | null): Promise<Found[]> {
  if (native) return (await native.detect(source)).map((b) => ({ text: b.rawValue, corners: b.cornerPoints }));
  const image = source instanceof ImageData ? source : pixels(source);
  // Grids can hold many codes; ask for all of them.
  const results = await readBarcodes(image, { formats: ["QRCode"], maxNumberOfSymbols: 255, tryHarder: true });
  return results
    .filter((r) => r.isValid)
    .map((r) => ({ text: r.text, corners: [r.position.topLeft, r.position.topRight, r.position.bottomRight, r.position.bottomLeft] }));
}

let crop: OffscreenCanvas | undefined;
const CROP = 192;

/**
 * How much fine detail the middle of the picture has: the mean squared
 * difference between each pixel and its neighbors (a blurred picture has
 * little). Only comparable between pictures of the same scene.
 */
function sharpness(bitmap: ImageBitmap): number {
  crop ??= new OffscreenCanvas(CROP, CROP);
  const ctx = crop.getContext("2d", { willReadFrequently: true })!;
  const size = Math.min(CROP, bitmap.width, bitmap.height);
  ctx.drawImage(bitmap, (bitmap.width - size) / 2, (bitmap.height - size) / 2, size, size, 0, 0, size, size);
  const d = ctx.getImageData(0, 0, size, size).data;
  const gray = (i: number) => d[i] + d[i + 1] * 2 + d[i + 2];
  let sum = 0;
  for (let y = 1; y < size - 1; y++) {
    for (let x = 1; x < size - 1; x++) {
      const i = (y * size + x) * 4;
      const v = 4 * gray(i) - gray(i - 4) - gray(i + 4) - gray(i - size * 4) - gray(i + size * 4);
      sum += v * v;
    }
  }
  return sum / ((size - 2) * (size - 2));
}

/**
 * Color codes stack three codes as the red, green and blue parts of the
 * picture. Returns what each part holds, read as a gray picture of its own.
 */
async function detectColors(bitmap: ImageBitmap, native: BarcodeDetector | null): Promise<Found[][]> {
  const rgba = pixels(bitmap).data;
  const out: Found[][] = [];
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
// What the camera makes of the three colors, over the last pictures taken
// apart in which anything was read: in which parts codes were found, and
// which pairs of parts held the same codes. The sender is told (bits 0-2:
// red, green, blue are read; bits 3-5: red and green, red and blue, green
// and blue look alike), and when it is showing color codes it keeps to the
// colors this camera tells apart. Zero: nothing to say yet.
const PAIRS = [
  [0, 1],
  [0, 2],
  [1, 2],
] as const;
interface ColorsSeen {
  read: boolean[];
  /** Per pair: both parts were read, and whether they shared a code. */
  both: boolean[];
  alike: boolean[];
}
const seen: ColorsSeen[] = [];
const SEEN_REMEMBERED = 8;
let colors = 0;

function noteColors(parts: Found[][]): void {
  const read = parts.map((p) => p.length > 0);
  if (!read.some(Boolean)) return;
  const texts = parts.map((p) => new Set(p.map((f) => f.text)));
  const both = PAIRS.map(([a, b]) => read[a] && read[b]);
  const alike = PAIRS.map(([a, b], n) => both[n] && [...texts[a]].some((t) => texts[b].has(t)));
  seen.push({ read, both, alike });
  if (seen.length > SEEN_REMEMBERED) seen.shift();
  if (seen.length < SEEN_REMEMBERED) {
    colors = 0;
    return;
  }
  let bits = 0;
  // A part counts as read when it was in a fair share of the pictures (a
  // weak camera loses any part now and then)…
  for (let c = 0; c < 3; c++) if (seen.filter((s) => s.read[c]).length * 3 >= seen.length) bits |= 1 << c;
  // …and two parts as alike when they mostly were, whenever both were read.
  PAIRS.forEach((_, n) => {
    const together = seen.filter((s) => s.both[n]);
    if (together.length >= 2 && together.filter((s) => s.alike[n]).length * 2 > together.length) bits |= 8 << n;
  });
  colors = bits;
}

/** Only one part held codes in the picture last taken apart. */
const oneColorLast = () => seen.length > 0 && seen[seen.length - 1].read.filter(Boolean).length === 1;

// What the sender said about its colors (see `declare` below): with two or
// more in use, every picture is read color by color whatever it looks like.
// To a camera that does not keep the colors apart, color codes look black
// and white; going by looks, the receiver would read one picture in twelve
// by color and never find out what it is missing.
let declared = -1;
const declaredColored = () => declared > 0 && (declared & (declared - 1)) !== 0;

async function scan(bitmap: ImageBitmap, native: BarcodeDetector | null): Promise<Found[]> {
  sinceProbe++;
  // (Sooner again after a picture with codes in one part only: that is how
  // color codes look to a camera that cannot keep the colors apart, and the
  // sender should hear of it before long.)
  const every = oneColorLast() ? COLOR_PROBE_EVERY / 4 : COLOR_PROBE_EVERY;
  if (!colored && !declaredColored() && sinceProbe < every) return detect(bitmap, native);
  sinceProbe = 0;
  const parts = await detectColors(bitmap, native);
  const all = new Map<string, Found>();
  for (const found of parts.flat()) all.set(found.text, found);
  const read = parts.filter((p) => p.length > 0);
  const most = Math.max(...parts.map((p) => p.length));
  if (all.size > most) {
    colored = true;
  } else if (read.length >= 2) {
    // Every part that could be read holds the same codes.
    colored = false;
  }
  noteColors(parts);
  return [...all.values()];
}

/** Whether to report where the codes are and how sharp the picture is (for advice on holding the camera). */
let guiding = false;

self.onmessage = async (e: MessageEvent<{ bitmap?: ImageBitmap; zxing?: boolean; guide?: boolean; declared?: number }>) => {
  if (e.data.declared !== undefined) {
    declared = e.data.declared;
    return;
  }
  if (e.data.guide !== undefined) {
    guiding = e.data.guide;
    return;
  }
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
    const found = await scan(bitmap, native);
    texts = found.map((f) => f.text);
    const ms = performance.now() - started;
    const look = guiding
      ? {
          width: bitmap.width,
          height: bitmap.height,
          boxes: found.map(box).filter((b) => b !== undefined),
          sharp: sharpness(bitmap),
        }
      : undefined;
    self.postMessage({ texts, engine, colored, colors, ms, look });
  } catch (err) {
    self.postMessage({ texts: [], engine, colored, error: String(err) });
  } finally {
    bitmap.close();
  }
};
