// Finds the QR codes in a picture: with the platform's BarcodeDetector where
// it reads QR codes (Chrome on Android and macOS, …), otherwise with ZXing
// (WebAssembly; Safari, Firefox, iPhone). Used in the scanner's worker, or on
// the page itself.

import { prepareZXingModule, readBarcodes } from "zxing-wasm/reader";

declare class BarcodeDetector {
  constructor(options: { formats: string[] });
  static getSupportedFormats(): Promise<string[]>;
  detect(source: ImageBitmapSource): Promise<{ rawValue: string }[]>;
}

export interface DetectOptions {
  /**
   * Where ZXing's WebAssembly (`zxing_reader.wasm` of the `zxing-wasm`
   * package) is served from. Left out, ZXing fetches it from the jsDelivr
   * CDN, and only on browsers without a BarcodeDetector for QR codes.
   */
  zxingWasmUrl?: string;
  /** Use ZXing even where the platform has a detector. */
  zxingOnly?: boolean;
}

export type Engine = "BarcodeDetector" | "ZXing";

export interface Detector {
  readonly engine: Engine;
  detect(picture: ImageBitmap | ImageData): Promise<string[]>;
}

let prepared: string | undefined;

export async function createDetector(options: DetectOptions = {}): Promise<Detector> {
  if (!options.zxingOnly && "BarcodeDetector" in globalThis) {
    try {
      if ((await BarcodeDetector.getSupportedFormats()).includes("qr_code")) {
        const native = new BarcodeDetector({ formats: ["qr_code"] });
        return {
          engine: "BarcodeDetector",
          detect: async (picture) => (await native.detect(picture)).map((b) => b.rawValue),
        };
      }
    } catch {
      /* not usable: ZXing instead */
    }
  }
  const url = options.zxingWasmUrl;
  if (url && prepared !== url) {
    prepared = url;
    prepareZXingModule({
      overrides: { locateFile: (path: string, prefix: string) => (path.endsWith(".wasm") ? url : prefix + path) },
    });
  }
  let canvas: OffscreenCanvas | undefined;
  const pixels = (bitmap: ImageBitmap): ImageData => {
    if (!canvas || canvas.width !== bitmap.width || canvas.height !== bitmap.height) {
      canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    }
    const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
    ctx.drawImage(bitmap, 0, 0);
    return ctx.getImageData(0, 0, bitmap.width, bitmap.height);
  };
  return {
    engine: "ZXing",
    detect: async (picture) => {
      const image = picture instanceof ImageData ? picture : pixels(picture);
      // A picture can hold many codes: ask for all of them.
      const results = await readBarcodes(image, { formats: ["QRCode"], maxNumberOfSymbols: 255, tryHarder: true });
      return results.filter((r) => r.isValid).map((r) => r.text);
    },
  };
}
