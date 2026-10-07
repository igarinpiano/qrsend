// Draws QR module matrices onto a canvas, crisp (integer module size).
// Codes in a grid share one quiet zone, like the CLI's layout.

export interface Matrix {
  width: number;
  modules: Uint8Array;
}

/** Columns and rows that fit `w × h` pixels with modules of at least `minPx`. */
export function fitGrid(modules: number, quiet: number, w: number, h: number, minPx: number): [number, number] {
  const fit = (px: number) => Math.max(1, Math.floor((Math.floor(px / minPx) - quiet) / (modules + quiet)));
  return [fit(w), fit(h)];
}

/**
 * Draws `count` codes of `modules × modules` from `data` (row-major, 1 = dark)
 * in a `cols × rows` grid centred on the canvas. Returns the module size in
 * device pixels.
 */
export function drawGrid(
  canvas: HTMLCanvasElement,
  data: Uint8Array,
  modules: number,
  count: number,
  cols: number,
  rows: number,
  quiet: number,
): number {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.floor(canvas.clientWidth * dpr);
  const h = Math.floor(canvas.clientHeight * dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d", { alpha: false })!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, w, h);
  if (count === 0) return 0;
  const extW = cols * (modules + quiet) + quiet;
  const extH = rows * (modules + quiet) + quiet;
  const scale = Math.max(1, Math.floor(Math.min(w / extW, h / extH)));
  const x0 = Math.floor((w - extW * scale) / 2);
  const y0 = Math.floor((h - extH * scale) / 2);
  const pitch = (modules + quiet) * scale;
  ctx.fillStyle = "#000";
  for (let n = 0; n < Math.min(count, cols * rows); n++) {
    const ox = x0 + quiet * scale + (n % cols) * pitch;
    const oy = y0 + quiet * scale + Math.floor(n / cols) * pitch;
    const base = n * modules * modules;
    for (let y = 0; y < modules; y++) {
      let run = -1;
      const row = base + y * modules;
      for (let x = 0; x <= modules; x++) {
        const dark = x < modules && data[row + x] === 1;
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

/** Fill styles by which of the three stacked codes is dark: bit 0 red, bit 1 green, bit 2 blue. */
const COLOURS = ["#fff", "#0ff", "#f0f", "#00f", "#ff0", "#0f0", "#f00", "#000"];

/**
 * Like `drawGrid`, but every cell stacks three codes: one in each of the red,
 * green and blue parts of the picture (a dark module switches that part off).
 * `data` holds `3 × cells` codes; cell `n` shows codes `3n`, `3n+1`, `3n+2`.
 */
export function drawColourGrid(
  canvas: HTMLCanvasElement,
  data: Uint8Array,
  modules: number,
  count: number,
  cols: number,
  rows: number,
  quiet: number,
): number {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.floor(canvas.clientWidth * dpr);
  const h = Math.floor(canvas.clientHeight * dpr);
  if (canvas.width !== w || canvas.height !== h) {
    canvas.width = w;
    canvas.height = h;
  }
  const ctx = canvas.getContext("2d", { alpha: false })!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, w, h);
  const cells = Math.min(Math.floor(count / 3), cols * rows);
  if (cells === 0) return 0;
  const extW = cols * (modules + quiet) + quiet;
  const extH = rows * (modules + quiet) + quiet;
  const scale = Math.max(1, Math.floor(Math.min(w / extW, h / extH)));
  const x0 = Math.floor((w - extW * scale) / 2);
  const y0 = Math.floor((h - extH * scale) / 2);
  const pitch = (modules + quiet) * scale;
  const size = modules * modules;
  for (let n = 0; n < cells; n++) {
    const ox = x0 + quiet * scale + (n % cols) * pitch;
    const oy = y0 + quiet * scale + Math.floor(n / cols) * pitch;
    const base = n * 3 * size;
    for (let y = 0; y < modules; y++) {
      const row = y * modules;
      let run = 0;
      let current = 0;
      for (let x = 0; x <= modules; x++) {
        const at = base + row + x;
        const colour = x < modules ? data[at] | (data[at + size] << 1) | (data[at + 2 * size] << 2) : -1;
        if (colour === current) continue;
        if (current > 0) {
          ctx.fillStyle = COLOURS[current];
          ctx.fillRect(ox + run * scale, oy + y * scale, (x - run) * scale, scale);
        }
        run = x;
        current = colour;
      }
    }
  }
  return scale;
}

/** Renders a single matrix to a data URL (device ID codes). */
export function toDataUrl(code: Matrix, scale = 6): string {
  const quiet = 4;
  const canvas = document.createElement("canvas");
  const side = (code.width + 2 * quiet) * scale;
  canvas.width = canvas.height = side;
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, side, side);
  ctx.fillStyle = "#000";
  for (let y = 0; y < code.width; y++)
    for (let x = 0; x < code.width; x++)
      if (code.modules[y * code.width + x]) ctx.fillRect((x + quiet) * scale, (y + quiet) * scale, scale, scale);
  return canvas.toDataURL("image/png");
}
