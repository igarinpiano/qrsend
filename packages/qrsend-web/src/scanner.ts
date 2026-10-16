// Reads the codes of a transfer with a camera and puts the transfer together.
//
// QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in
// other countries. A page that reads these codes and names them should say
// so (see README.md, "Trademark"; the sentence is `QR_CODE_TRADEMARK`).

import { createDetector, type DetectOptions, type Detector, type Engine } from "./detect.js";
import { Decoder, type Progress, type Received } from "./index.js";

export { QR_CODE_TRADEMARK } from "./index.js";
export type { DetectOptions, Engine } from "./detect.js";

export interface ScannerOptions extends DetectOptions {
  /** The camera to use (from `QrScanner.cameras()`). */
  deviceId?: string;
  /** Without `deviceId`: "environment" (default, the one on the back) or "user" (the one facing the user). */
  facingMode?: "environment" | "user";
  /** Read this stream instead of opening a camera: a shared screen, a `canvas.captureStream()`, … */
  stream?: MediaStream;
  /** Find the codes on a worker, off the page's main thread (default true). */
  worker?: boolean;
}

type VideoWithFrames = HTMLVideoElement & {
  requestVideoFrameCallback?: (callback: () => void) => number;
};

/**
 * Reads a transfer shown as QR codes (by `QrPlayer`, or the QRSend app or
 * CLI) with the camera, in a `<video>` element that shows what the camera
 * sees.
 *
 * ```js
 * const scanner = new QrScanner(document.querySelector("video"));
 * scanner.onprogress = (p) => (bar.value = p.ratio);
 * const received = await scanner.receive();
 * ```
 *
 * The page must be served over HTTPS (or from localhost) for the browser to
 * let it use the camera, and the browser asks the user first.
 */
export class QrScanner {
  /** Called when the transfer's progress changes. */
  onprogress?: (progress: Progress) => void;
  /** Called with the text of every code found in a picture, QRSend's or not. */
  oncodes?: (texts: string[]) => void;

  private stream?: MediaStream;
  private ownStream = false;
  private running = false;
  private busy = false;
  private turn = 0;
  private worker?: Worker;
  private local?: Promise<Detector>;
  private engineName?: Engine;
  private current?: Decoder;
  private waiting?: { resolve: (r: Received) => void; reject: (e: unknown) => void };

  constructor(
    readonly video: HTMLVideoElement,
    private options: ScannerOptions = {},
  ) {}

  /** The cameras of the device (their labels only once the user allowed one). */
  static async cameras(): Promise<MediaDeviceInfo[]> {
    return (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === "videoinput");
  }

  /** What finds the codes, once the first picture has been read. */
  get engine(): Engine | undefined {
    return this.engineName;
  }

  /** The transfer being put together (`progress`, `entries()`), once `receive()` was called. */
  get decoder(): Decoder | undefined {
    return this.current;
  }

  get scanning(): boolean {
    return this.running;
  }

  /**
   * Opens the camera (unless it is open) and reads until the transfer is
   * complete; then closes it and resolves with what came. `stop()` rejects it
   * with an "AbortError", and a later `receive()` goes on where it was.
   */
  async receive(): Promise<Received> {
    this.current ??= await Decoder.create();
    this.waiting?.reject(new DOMException("receive() was called again", "AbortError"));
    const done = new Promise<Received>((resolve, reject) => (this.waiting = { resolve, reject }));
    if (!this.running) await this.start();
    return done;
  }

  /** Forgets the transfer being put together, to read another one from the start. */
  reset(): void {
    this.current?.free();
    this.current = undefined;
  }

  /** Opens the camera and starts reading codes (`oncodes`). `receive()` calls it. */
  async start(): Promise<void> {
    this.close();
    const turn = this.turn;
    let stream = this.options.stream;
    if (!stream) {
      const camera: MediaTrackConstraints = this.options.deviceId
        ? { deviceId: { exact: this.options.deviceId } }
        : { facingMode: { ideal: this.options.facingMode ?? "environment" } };
      stream = await navigator.mediaDevices.getUserMedia({
        audio: false,
        video: { ...camera, width: { ideal: 1920 }, height: { ideal: 1080 }, frameRate: { ideal: 30 } },
      });
      if (turn !== this.turn) {
        stream.getTracks().forEach((t) => t.stop());
        throw new DOMException("stopped before it started", "AbortError");
      }
      try {
        // Continuous autofocus where there is such a thing.
        await stream.getVideoTracks()[0]?.applyConstraints({ advanced: [{ focusMode: "continuous" } as MediaTrackConstraintSet] });
      } catch {
        /* not supported */
      }
    }
    this.stream = stream;
    this.ownStream = !this.options.stream;
    const v = this.video;
    v.srcObject = stream;
    v.muted = true;
    v.playsInline = true;
    await v.play();
    if (turn !== this.turn) throw new DOMException("stopped before it started", "AbortError");
    this.running = true;
    this.loop();
  }

  /** Stops reading and closes the camera. A `receive()` waiting rejects with an "AbortError". */
  stop(): void {
    this.close();
    const waiting = this.waiting;
    this.waiting = undefined;
    waiting?.reject(new DOMException("the scanner was stopped", "AbortError"));
  }

  /** Stops, and lets go of the worker and of the transfer being put together. */
  dispose(): void {
    this.stop();
    this.worker?.terminate();
    this.worker = undefined;
    this.reset();
  }

  private close(): void {
    this.turn++;
    this.running = false;
    this.busy = false;
    if (this.ownStream) this.stream?.getTracks().forEach((t) => t.stop());
    this.stream = undefined;
    this.video.srcObject = null;
  }

  private loop = (): void => {
    if (!this.running) return;
    const v = this.video as VideoWithFrames;
    if (!this.busy && v.readyState >= 2 && v.videoWidth > 0) this.read(v);
    if (v.requestVideoFrameCallback) v.requestVideoFrameCallback(this.loop);
    else requestAnimationFrame(this.loop);
  };

  private async read(v: HTMLVideoElement): Promise<void> {
    this.busy = true;
    const turn = this.turn;
    try {
      const picture = await createImageBitmap(v);
      const texts = await this.detect(picture);
      if (turn === this.turn) this.found(texts);
    } catch {
      /* this picture is lost; the next one comes */
    } finally {
      if (turn === this.turn) this.busy = false;
    }
  }

  private detect(picture: ImageBitmap): Promise<string[]> {
    if (this.options.worker === false || this.local) return this.detectHere(picture);
    if (!this.worker) {
      try {
        this.worker = new Worker(new URL("./scan-worker.js", import.meta.url), { type: "module" });
        const { deviceId: _d, facingMode: _f, stream: _s, worker: _w, ...detect } = this.options;
        this.worker.postMessage({ options: detect });
      } catch {
        return this.detectHere(picture);
      }
    }
    const worker = this.worker;
    return new Promise<string[]>((resolve) => {
      worker.onmessage = (e: MessageEvent<{ texts: string[]; engine?: Engine }>) => {
        if (e.data.engine) this.engineName = e.data.engine;
        resolve(e.data.texts);
      };
      // The worker could not be loaded (a bundler that left it out, a
      // strict Content-Security-Policy): find the codes here instead.
      worker.onerror = (e) => {
        e.preventDefault();
        worker.terminate();
        if (this.worker === worker) this.worker = undefined;
        this.local ??= createDetector(this.options);
        resolve([]);
      };
      worker.postMessage({ picture }, [picture]);
    });
  }

  private async detectHere(picture: ImageBitmap): Promise<string[]> {
    try {
      this.local ??= createDetector(this.options);
      const d = await this.local;
      this.engineName = d.engine;
      return await d.detect(picture);
    } finally {
      picture.close();
    }
  }

  private found(texts: string[]): void {
    if (!texts.length) return;
    this.oncodes?.(texts);
    const decoder = this.current;
    if (!decoder || !this.waiting || !decoder.push(texts)) return;
    this.onprogress?.(decoder.progress);
    if (!decoder.complete) return;
    const waiting = this.waiting;
    this.waiting = undefined;
    this.close();
    try {
      waiting.resolve(decoder.result());
    } catch (e) {
      waiting.reject(e);
    } finally {
      this.reset();
    }
  }
}

/**
 * The texts of the QR codes in one picture (an `<img>`, a `<canvas>`, a
 * `Blob` of an image, …): for codes that come some other way than a camera.
 * Pass them to `Decoder.push`.
 */
export async function readCodes(picture: ImageBitmapSource | ImageData, options: DetectOptions = {}): Promise<string[]> {
  const detector = await createDetector(options);
  if (picture instanceof ImageData) return detector.detect(picture);
  const bitmap = await createImageBitmap(picture);
  try {
    return await detector.detect(bitmap);
  } finally {
    bitmap.close();
  }
}
