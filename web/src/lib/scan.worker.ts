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
    if (native) {
      engine = "native";
      texts = (await native.detect(bitmap)).map((b) => b.rawValue);
    } else {
      if (!canvas || canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
        canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      }
      const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
      ctx.drawImage(bitmap, 0, 0);
      const image = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
      // Grids can hold many codes; ask for all of them.
      const results = await readBarcodes(image, { formats: ["QRCode"], maxNumberOfSymbols: 255, tryHarder: true });
      texts = results.filter((r) => r.isValid).map((r) => r.text);
    }
    self.postMessage({ texts, engine });
  } catch (err) {
    self.postMessage({ texts: [], engine, error: String(err) });
  } finally {
    bitmap.close();
  }
};
