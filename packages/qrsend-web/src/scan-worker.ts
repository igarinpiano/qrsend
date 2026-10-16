/// <reference lib="webworker" />
// Reads the codes in the scanner's pictures off the page's main thread.

import { createDetector, type DetectOptions, type Detector } from "./detect.js";

type Request = { options: DetectOptions } | { picture: ImageBitmap };

let detector: Promise<Detector> | undefined;

self.onmessage = async (e: MessageEvent<Request>) => {
  if ("options" in e.data) {
    detector = createDetector(e.data.options);
    return;
  }
  const { picture } = e.data;
  try {
    detector ??= createDetector();
    const d = await detector;
    const texts = await d.detect(picture);
    self.postMessage({ texts, engine: d.engine });
  } catch (error) {
    self.postMessage({ texts: [], error: String(error) });
  } finally {
    picture.close();
  }
};
