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
  /** Pictures per second (default 10). Slower is easier for a weak camera; faster sends more. */
  fps?: number;
  /**
   * How many codes each picture holds: a number (default 1), `[columns,
   * rows]`, or "fill" for as many as fit with modules of at least
   * `minModulePixels`. More codes send more, but each comes out smaller.
   */
  grid?: number | readonly [number, number] | "fill";
  /** With `grid: "fill"`: the smallest module, in device pixels (default 4). */
  minModulePixels?: number;
}

export interface PictureInfo {
  /** Codes shown so far. */
  count: number;
  /** Codes that hold everything once. */
  codesPerRound: number;
  /** Codes in this picture. */
  codes: number;
  /** Device pixels per module. */
  modulePixels: number;
}

/**
 * Shows a transfer on a `<canvas>` as QR codes, for another device's camera
 * to read (with `QrScanner` from `@qrsend/web/scanner`, or the QRSend app).
 * It goes on until `stop()`. Give the canvas its size with CSS; the codes are
 * drawn sharp at the device's pixel ratio, centered, on white.
 *
 * ```js
 * const player = new QrPlayer(document.querySelector("canvas"), { fps: 10 });
 * await player.play(file);
 * ```
 */
export class QrPlayer {
  /** Called after each picture. */
  onpicture?: (info: PictureInfo) => void;

  private encoder?: Encoder;
  private frame = 0;
  private last = 0;
  private options: PlayerOptions;
  private turn = 0;

  constructor(
    readonly canvas: HTMLCanvasElement,
    options: PlayerOptions = {},
  ) {
    this.options = { ...options };
  }

  /** Pictures per second. Can be changed while playing. */
  get fps(): number {
    return this.options.fps ?? 10;
  }

  set fps(value: number) {
    this.options.fps = value;
  }

  get grid(): PlayerOptions["grid"] {
    return this.options.grid ?? 1;
  }

  set grid(value: PlayerOptions["grid"]) {
    this.options.grid = value;
  }

  get playing(): boolean {
    return this.frame !== 0;
  }

  /** The encoder of what is playing (its size, `codesPerRound`, …). */
  get current(): Encoder | undefined {
    return this.encoder;
  }

  /**
   * Packs `content` and starts showing it (replacing whatever played).
   * Resolves once the first picture is drawn.
   */
  async play(content: Content, options: EncodeOptions = {}): Promise<Encoder> {
    this.stop();
    const turn = this.turn;
    const encoder = await Encoder.create(content, { ...this.options, ...options });
    if (turn !== this.turn) {
      encoder.free();
      throw new DOMException("stopped before it started", "AbortError");
    }
    this.encoder = encoder;
    this.last = 0;
    this.draw();
    this.frame = requestAnimationFrame(this.tick);
    return encoder;
  }

  /** Stops, clears the canvas and frees what was playing. */
  stop(): void {
    this.turn++;
    if (this.frame) cancelAnimationFrame(this.frame);
    this.frame = 0;
    this.encoder?.free();
    this.encoder = undefined;
    const ctx = this.canvas.getContext("2d");
    ctx?.clearRect(0, 0, this.canvas.width, this.canvas.height);
  }

  private tick = (now: number): void => {
    if (!this.encoder) return;
    const interval = 1000 / Math.max(0.1, this.fps);
    if (now - this.last >= interval) {
      // Keep to the rate on average, without catching up after a pause.
      this.last = now - this.last > 2 * interval ? now : this.last + interval;
      this.draw();
    }
    this.frame = requestAnimationFrame(this.tick);
  };

  private draw(): void {
    const encoder = this.encoder!;
    const canvas = this.canvas;
    const dpr = globalThis.devicePixelRatio || 1;
    const w = Math.round((canvas.clientWidth || canvas.width) * dpr);
    const h = Math.round((canvas.clientHeight || canvas.height) * dpr);
    if (w === 0 || h === 0) return;
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
    }
    const size = encoder.size;
    const [cols, rows] = layout(this.grid, size, w, h, this.options.minModulePixels ?? 4);
    const codes: Uint8Array[] = [];
    for (let i = 0; i < cols * rows; i++) codes.push(encoder.nextModules());
    const modulePixels = drawCodes(canvas, codes, size, cols, rows);
    this.onpicture?.({ count: encoder.count, codesPerRound: encoder.codesPerRound, codes: codes.length, modulePixels });
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
