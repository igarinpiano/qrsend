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
 * in a `cols × rows` grid centered on the canvas. Returns the module size in
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
const COLORS = ["#fff", "#0ff", "#f0f", "#00f", "#ff0", "#0f0", "#f00", "#000"];

/**
 * Like `drawGrid`, but every cell stacks several codes: one in each of the
 * colors in `channels` (0 red, 1 green, 2 blue; a dark module switches that
 * part of the picture off, and a color left out stays on throughout). `data`
 * holds `k × cells` codes for `k` colors; cell `n` shows codes `kn … kn+k-1`.
 */
export function drawColorGrid(
  canvas: HTMLCanvasElement,
  data: Uint8Array,
  modules: number,
  count: number,
  cols: number,
  rows: number,
  quiet: number,
  channels: readonly number[] = [0, 1, 2],
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
  const k = channels.length;
  const cells = Math.min(Math.floor(count / k), cols * rows);
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
    const base = n * k * size;
    for (let y = 0; y < modules; y++) {
      const row = y * modules;
      let run = 0;
      let current = 0;
      for (let x = 0; x <= modules; x++) {
        const at = base + row + x;
        let color = -1;
        if (x < modules) {
          color = 0;
          for (let j = 0; j < k; j++) color |= data[at + j * size] << channels[j];
        }
        if (color === current) continue;
        if (current > 0) {
          ctx.fillStyle = COLORS[current];
          ctx.fillRect(ox + run * scale, oy + y * scale, (x - run) * scale, scale);
        }
        run = x;
        current = color;
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
