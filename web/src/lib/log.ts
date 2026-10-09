// A diagnostic log: what happened when, in plain text, for pasting into a
// bug report ("Copy log" at the bottom of every page).
//
// It records how a transfer went (steps, timings, counts, the states of a
// connection), never what was transferred: no file names, no text, no
// device IDs, no network addresses. It lives in memory; the last session's
// log is also kept in this browser's storage, so that it can still be copied
// after a reload.

declare const __APP_VERSION__: string;
declare const __BUILD_TIME__: string;

type Value = string | number | boolean | null | undefined;

const MAX_LINES = 2000;
/** Kept whatever follows: how the session began. */
const KEEP_FIRST = 60;
const STORE = "qrsend.log.last";

const lines: string[] = [];
let dropped = 0;
const began = performance.now();
const beganAt = new Date();
let previous = "";
let dirty = false;

function show(value: Value): string {
  if (typeof value === "number") return Number.isInteger(value) ? String(value) : value.toFixed(Math.abs(value) < 10 ? 2 : 1);
  if (typeof value === "string") return /^[\w.:%×+-]*$/.test(value) ? value : JSON.stringify(value);
  return String(value);
}

/** Records one event. `area` says which part speaks (rx, tx, camera, link…). */
/** `localStorage["qrsend.debug.log"] = "1"`: every line also goes to the console as it is written (for tests). */
const mirrored = (() => {
  try {
    return localStorage.getItem("qrsend.debug.log") === "1";
  } catch {
    return false;
  }
})();

export function log(area: string, what: string, data: Record<string, Value> = {}): void {
  const at = ((performance.now() - began) / 1000).toFixed(2).padStart(8);
  const details = Object.entries(data)
    .filter(([, v]) => v !== undefined)
    .map(([k, v]) => `${k}=${show(v)}`)
    .join(" ");
  lines.push(`${at} ${area.padEnd(6)} ${what}${details ? `  ${details}` : ""}`);
  if (mirrored) console.debug(`[qrsend] ${lines[lines.length - 1]}`);
  if (lines.length > MAX_LINES) {
    lines.splice(KEEP_FIRST, 1);
    dropped++;
  }
  dirty = true;
}

const last = new Map<string, number>();

/** Like `log`, but at most once per `everyMs` for each `key`: for readings taken over and over. */
export function logEvery(key: string, everyMs: number, area: string, what: string, data: () => Record<string, Value>): void {
  const now = performance.now();
  if (now - (last.get(key) ?? -Infinity) < everyMs) return;
  last.set(key, now);
  log(area, what, data());
}

function header(): string[] {
  const features: string[] = [];
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i) ?? "";
      // (On, or the way it is on: "lan=wifi".)
      const value = key.startsWith("qrsend.preview.") ? localStorage.getItem(key) : null;
      if (value) features.push(value === "1" ? key.slice(15) : `${key.slice(15)}=${value}`);
    }
  } catch {
    /* no storage */
  }
  const nav = navigator as Navigator & { deviceMemory?: number; connection?: { effectiveType?: string; type?: string } };
  return [
    `QRSend ${__APP_VERSION__} (built ${__BUILD_TIME__}) · log of ${beganAt.toISOString()}`,
    `browser: ${navigator.userAgent}`,
    `screen: ${screen.width}×${screen.height} at ${window.devicePixelRatio}x · window ${window.innerWidth}×${window.innerHeight} · cores ${
      navigator.hardwareConcurrency ?? "?"
    } · memory ${nav.deviceMemory ?? "?"} GB · network ${nav.connection?.type ?? "?"} (as fast as ${
      nav.connection?.effectiveType ?? "?"
    })`,
    `preview features on: ${features.sort().join(", ") || "none"}`,
    "(No file names, contents, device IDs or network addresses are recorded.)",
  ];
}

/** The whole log as text. */
export function logText(): string {
  const body = dropped ? [...lines.slice(0, KEEP_FIRST), `         … ${dropped} lines left out …`, ...lines.slice(KEEP_FIRST)] : lines;
  const out = [...header(), "", ...body];
  if (previous) out.push("", "--- the session before this one (same browser tab or its predecessor) ---", previous);
  return out.join("\n");
}

export const logLines = () => lines.length;

function keep(): void {
  if (!dirty) return;
  dirty = false;
  try {
    // The end matters most; storage is small.
    localStorage.setItem(STORE, [header()[0], ...lines.slice(-600)].join("\n"));
  } catch {
    /* no storage, or full: the log stays in memory */
  }
}

if (typeof window !== "undefined") {
  try {
    previous = localStorage.getItem(STORE) ?? "";
    localStorage.removeItem(STORE);
  } catch {
    /* no storage */
  }
  setInterval(keep, 5000);
  window.addEventListener("pagehide", keep);
  window.addEventListener("error", (e) => log("error", e.message || "error", { at: `${(e.filename ?? "").split("/").pop()}:${e.lineno}` }));
  window.addEventListener("unhandledrejection", (e) =>
    log("error", "unhandled", { reason: String((e.reason as Error | undefined)?.message ?? e.reason).slice(0, 200) }),
  );
  document.addEventListener("visibilitychange", () => log("app", document.visibilityState));
}
