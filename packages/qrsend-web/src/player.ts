// Shows the codes of a transfer on a canvas, one picture after another.
//
// QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in
// other countries. A page that shows these codes should say so (see
// README.md, "Trademark"; the sentence is `QR_CODE_TRADEMARK`).

import { Encoder, type Content, type EncodeOptions } from "./index.js";

export { QR_CODE_TRADEMARK } from "./index.js";

/** Modules of white between codes and around them. */
const QUIET = 4;

export interface PlayerOptions extends EncodeOptions {
  /**
   * How long each picture stays, in milliseconds (default 100: ten pictures a
   * second). Longer is easier for a weak camera; shorter sends more. A
   * picture cannot change more often than the display does (about 16 ms at
   * 60 Hz). Can be changed while playing (`interval`).
   */
  interval?: number;
  /**
   * How many codes each picture holds: a number (default 1), `[columns,
   * rows]`, or "fill" for as many as fit with modules of at least
   * `minModulePixels`. More codes send more, but each comes out smaller.
   */
  grid?: number | readonly [number, number] | "fill";
  /** With `grid: "fill"`: the smallest module, in device pixels (default 4). */
  minModulePixels?: number;
  /** How many of the pictures shown are kept for stepping back (default 300). */
  history?: number;
}

export interface PictureInfo {
  /** The number of the picture on the canvas: 1 for the first one shown. */
  picture: number;
  /** Pictures made so far (the picture on the canvas is an earlier one after stepping back). */
  pictures: number;
  /** Codes made so far. */
  count: number;
  /** Codes that hold everything once. */
  codesPerRound: number;
  /** Codes in this picture. */
  codes: number;
  /** Device pixels per module. */
  modulePixels: number;
  paused: boolean;
}

/** A picture shown: the texts of its codes, so that it can be shown again. */
interface Picture {
  number: number;
  texts: string[];
  cols: number;
  rows: number;
}

/**
 * Shows a transfer on a `<canvas>` as QR codes, for another device's camera
 * to read (with `QrScanner` from `@qrsend/web/scanner`, or the QRSend app).
 * It goes on until `stop()`; it can be paused, stepped picture by picture
 * (also back to pictures shown before) and sped up or slowed down. Give the
 * canvas its size with CSS; the codes are drawn sharp at the device's pixel
 * ratio, centered, on white.
 *
 * ```js
 * const player = new QrPlayer(document.querySelector("canvas"), { interval: 100 });
 * await player.play(file);
 * player.pause();
 * player.stepBack();
 * player.interval = 250;
 * player.resume();
 * ```
 */
export class QrPlayer {
  /** Called when a picture is drawn, and when playing is paused or resumed. */
  onpicture?: (info: PictureInfo) => void;

  private encoder?: Encoder;
  private frame = 0;
  private last = 0;
  private options: PlayerOptions;
  private turn = 0;
  private isPaused = false;
  /** The pictures shown, oldest first, and which of them is on the canvas. */
  private shown: Picture[] = [];
  private at = -1;
  private made = 0;
  /** The canvas size the picture on it was drawn for. */
  private drawnFor = "";

  constructor(
    readonly canvas: HTMLCanvasElement,
    options: PlayerOptions = {},
  ) {
    this.options = { ...options };
  }

  /** Milliseconds each picture stays. Can be changed while playing. */
  get interval(): number {
    return this.options.interval ?? 100;
  }

  set interval(ms: number) {
    if (!(ms > 0)) throw new RangeError("the interval must be more than 0 ms");
    this.options.interval = ms;
  }

  /** Codes per picture, for the pictures made from now on. */
  get grid(): PlayerOptions["grid"] {
    return this.options.grid ?? 1;
  }

  set grid(value: PlayerOptions["grid"]) {
    this.options.grid = value;
  }

  /** Whether something is loaded (playing or paused). */
  get playing(): boolean {
    return this.encoder !== undefined;
  }

  get paused(): boolean {
    return this.isPaused;
  }

  /** The encoder of what is loaded (its size, `codesPerRound`, …). */
  get current(): Encoder | undefined {
    return this.encoder;
  }

  /**
   * Packs `content` and starts showing it (replacing whatever played).
   * Resolves once the first picture is drawn. With `{ paused: true }` it
   * stays on the first picture until `resume()` or a step.
   */
  async play(content: Content, options: EncodeOptions & { paused?: boolean } = {}): Promise<Encoder> {
    this.stop();
    const turn = this.turn;
    const encoder = await Encoder.create(content, { ...this.options, ...options });
    if (turn !== this.turn) {
      encoder.free();
      throw new DOMException("stopped before it started", "AbortError");
    }
    this.encoder = encoder;
    this.isPaused = options.paused ?? false;
    this.last = performance.now();
    this.forward();
    this.frame = requestAnimationFrame(this.tick);
    return encoder;
  }

  /** Stays on the picture shown. */
  pause(): void {
    this.isPaused = true;
    this.report();
  }

  /** Goes on from the picture shown (through the pictures stepped back over, then new ones). */
  resume(): void {
    if (!this.isPaused) return;
    this.isPaused = false;
    this.last = performance.now();
    this.report();
  }

  /** Pauses, or resumes when paused. */
  toggle(): void {
    if (this.isPaused) this.resume();
    else this.pause();
  }

  /** Pauses and shows the next picture. */
  stepForward(): void {
    if (!this.encoder) return;
    this.isPaused = true;
    this.forward();
  }

  /** Pauses and shows the picture before. Returns false at the oldest picture kept (`history`). */
  stepBack(): boolean {
    if (!this.encoder) return false;
    this.isPaused = true;
    if (this.at <= 0) {
      this.report();
      return false;
    }
    this.at--;
    this.draw();
    return true;
  }

  /** Stops, clears the canvas and frees what was playing. */
  stop(): void {
    this.turn++;
    if (this.frame) cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.encoder?.free();
    this.encoder = undefined;
    this.shown = [];
    this.at = -1;
    this.made = 0;
    this.drawnFor = "";
    this.isPaused = false;
    const ctx = this.canvas.getContext("2d");
    ctx?.clearRect(0, 0, this.canvas.width, this.canvas.height);
  }

  private tick = (now: number): void => {
    if (!this.encoder) return;
    const interval = this.interval;
    if (!this.isPaused && now - this.last >= interval) {
      // Keep to the rate on average, without catching up after a pause.
      this.last = now - this.last > 2 * interval ? now : this.last + interval;
      this.forward();
    } else if (this.size() !== this.drawnFor) {
      // The canvas changed size: the same picture, drawn anew.
      this.draw();
    }
    this.frame = requestAnimationFrame(this.tick);
  };

  /** The next picture: one stepped back over, or a new one. */
  private forward(): void {
    if (this.at < this.shown.length - 1) {
      this.at++;
      this.draw();
      return;
    }
    const encoder = this.encoder!;
    const [w, h] = this.pixels();
    if (w === 0 || h === 0) return;
    const [cols, rows] = layout(this.grid, encoder.size, w, h, this.options.minModulePixels ?? 4);
    const texts: string[] = [];
    for (let i = 0; i < cols * rows; i++) texts.push(encoder.next());
    this.shown.push({ number: ++this.made, texts, cols, rows });
    const keep = Math.max(1, Math.floor(this.options.history ?? 300));
    if (this.shown.length > keep) this.shown.splice(0, this.shown.length - keep);
    this.at = this.shown.length - 1;
    this.draw();
  }

  /** The canvas in device pixels. */
  private pixels(): [number, number] {
    const dpr = globalThis.devicePixelRatio || 1;
    const canvas = this.canvas;
    return [Math.round((canvas.clientWidth || canvas.width) * dpr), Math.round((canvas.clientHeight || canvas.height) * dpr)];
  }

  private size(): string {
    return this.pixels().join("x");
  }

  private draw(): void {
    const encoder = this.encoder!;
    const picture = this.shown[this.at];
    if (!picture) return;
    const canvas = this.canvas;
    const [w, h] = this.pixels();
    if (w === 0 || h === 0) return;
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    this.drawnFor = this.size();
    const codes = picture.texts.map((text) => encoder.render(text));
    this.modulePixels = drawCodes(canvas, codes, encoder.size, picture.cols, picture.rows);
    this.report();
  }

  private modulePixels = 0;

  private report(): void {
    const encoder = this.encoder;
    const picture = this.shown[this.at];
    if (!encoder || !picture) return;
    this.onpicture?.({
      picture: picture.number,
      pictures: this.made,
      count: encoder.count,
      codesPerRound: encoder.codesPerRound,
      codes: picture.texts.length,
      modulePixels: this.modulePixels,
      paused: this.isPaused,
    });
  }
}

/** Columns and rows of codes for a `w × h` canvas. */
function layout(grid: PlayerOptions["grid"], size: number, w: number, h: number, minPx: number): [number, number] {
  if (Array.isArray(grid)) return [Math.max(1, Math.floor(grid[0])), Math.max(1, Math.floor(grid[1]))];
  if (grid === "fill") {
    const fit = (px: number) => Math.max(1, Math.floor((Math.floor(px / Math.max(1, minPx)) - QUIET) / (size + QUIET)));
    return [fit(w), fit(h)];
  }
  // `n` codes: the arrangement that leaves them largest.
  const n = Math.max(1, Math.floor((grid as number | undefined) ?? 1));
  let best: [number, number] = [n, 1];
  let bestScale = 0;
  for (let cols = 1; cols <= n; cols++) {
    const rows = Math.ceil(n / cols);
    const scale = Math.min(w / (cols * (size + QUIET) + QUIET), h / (rows * (size + QUIET) + QUIET));
    if (scale > bestScale) {
      bestScale = scale;
      best = [cols, rows];
    }
  }
  return best;
}

/**
 * Draws codes (`size × size` modules each, 1 = dark) in a grid centered on
 * the canvas, with whole device pixels per module. Returns that number.
 */
export function drawCodes(
  canvas: HTMLCanvasElement | OffscreenCanvas,
  codes: readonly Uint8Array[],
  size: number,
  cols: number,
  rows: number,
): number {
  const { width: w, height: h } = canvas;
  const ctx = canvas.getContext("2d", { alpha: false }) as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, w, h);
  const extW = cols * (size + QUIET) + QUIET;
  const extH = rows * (size + QUIET) + QUIET;
  const scale = Math.max(1, Math.floor(Math.min(w / extW, h / extH)));
  const x0 = Math.floor((w - extW * scale) / 2);
  const y0 = Math.floor((h - extH * scale) / 2);
  const pitch = (size + QUIET) * scale;
  ctx.fillStyle = "#000";
  for (let n = 0; n < Math.min(codes.length, cols * rows); n++) {
    const code = codes[n];
    const ox = x0 + QUIET * scale + (n % cols) * pitch;
    const oy = y0 + QUIET * scale + Math.floor(n / cols) * pitch;
    for (let y = 0; y < size; y++) {
      // One rectangle per run of dark modules.
      let run = -1;
      for (let x = 0; x <= size; x++) {
        const dark = x < size && code[y * size + x] === 1;
        if (dark && run < 0) run = x;
        if (!dark && run >= 0) {
          ctx.fillRect(ox + run * scale, oy + y * scale, (x - run) * scale, scale);
          run = -1;
        }
      }
    }
  }
  return scale;
}
