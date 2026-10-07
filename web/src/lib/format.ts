export function bytes(n: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return u === 0 ? `${n} B` : `${v.toFixed(v < 10 ? 2 : 1)} ${units[u]}`;
}

export function duration(seconds: number): string {
  if (!isFinite(seconds)) return "—";
  const s = Math.round(seconds);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}

/** Transfer speed over a recent window, from samples of what is still missing. */
export class RateMeter {
  private samples: [number, number][] = [];
  constructor(private windowMs = 5000) {}

  /** Records `remaining` (bytes) at `now` (ms) and returns bytes per second. */
  update(now: number, remaining: number): number {
    this.samples.push([now, remaining]);
    while (this.samples.length > 2 && now - this.samples[0][0] > this.windowMs) this.samples.shift();
    const [t0, r0] = this.samples[0];
    const dt = (now - t0) / 1000;
    return dt > 0 ? Math.max(r0 - remaining, 0) / dt : 0;
  }
}
