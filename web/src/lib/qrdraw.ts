// Draws QR module matrices onto a canvas, crisp (integer module size).
export interface Matrix {
  width: number;
  modules: Uint8Array;
}

const QUIET = 4;

export function drawGrid(canvas: HTMLCanvasElement, codes: Matrix[], grid: number): void {
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
  if (codes.length === 0) return;
  const cellW = Math.floor(w / grid);
  const cellH = Math.floor(h / grid);
  const span = codes[0].width + 2 * QUIET;
  const scale = Math.max(1, Math.floor(Math.min(cellW, cellH) / span));
  const side = span * scale;
  ctx.fillStyle = "#000";
  codes.forEach((code, n) => {
    const ox = (n % grid) * cellW + Math.floor((cellW - side) / 2) + QUIET * scale;
    const oy = Math.floor(n / grid) * cellH + Math.floor((cellH - side) / 2) + QUIET * scale;
    const { width, modules } = code;
    for (let y = 0; y < width; y++) {
      let run = -1;
      for (let x = 0; x <= width; x++) {
        const dark = x < width && modules[y * width + x] === 1;
        if (dark && run < 0) run = x;
        if (!dark && run >= 0) {
          ctx.fillRect(ox + run * scale, oy + y * scale, (x - run) * scale, scale);
          run = -1;
        }
      }
    }
  });
}

/** Renders a single matrix to a data URL (device ID codes). */
export function toDataUrl(code: Matrix, scale = 6): string {
  const canvas = document.createElement("canvas");
  const side = (code.width + 2 * QUIET) * scale;
  canvas.width = canvas.height = side;
  const ctx = canvas.getContext("2d")!;
  ctx.fillStyle = "#fff";
  ctx.fillRect(0, 0, side, side);
  ctx.fillStyle = "#000";
  for (let y = 0; y < code.width; y++)
    for (let x = 0; x < code.width; x++)
      if (code.modules[y * code.width + x]) ctx.fillRect((x + QUIET) * scale, (y + QUIET) * scale, scale, scale);
  return canvas.toDataURL("image/png");
}
