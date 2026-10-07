// Shared helpers: the qrsend CLI binary and isolated environments for it.
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const BIN = process.env.QRSEND_BIN ?? path.resolve(import.meta.dirname, "../../target/release/qrsend");
export const WORK = path.join(os.tmpdir(), "qrsend-e2e");

/** Environment of one CLI "device" (own identity, inbox and cache). */
function env(device: string): NodeJS.ProcessEnv {
  return {
    ...process.env,
    QRSEND_DATA_DIR: path.join(WORK, device, "data"),
    QRSEND_CACHE_DIR: path.join(WORK, device, "cache"),
    QRSEND_CONFIG_DIR: path.join(WORK, device, "config"),
  };
}

export function cli(args: string[], device = "cli"): string {
  return execFileSync(BIN, args, { cwd: WORK, env: env(device), encoding: "utf8" });
}

/** Like `cli`, but also returns stderr (where the CLI reports the sender). */
export function cliBoth(args: string[], device = "cli"): { stdout: string; stderr: string; status: number | null } {
  const r = spawnSync(BIN, args, { cwd: WORK, env: env(device), encoding: "utf8" });
  return { stdout: r.stdout, stderr: r.stderr, status: r.status };
}

export const hasCli = fs.existsSync(BIN);
export const hasFfmpeg = spawnSync("ffmpeg", ["-version"]).status === 0;

export const FAKE_TEXT = "Hello from the CLI 👋 — QRSend e2e";
export const SECRET_TEXT = "For the browser only 🔐";
export const textVideo = path.join(WORK, "text.y4m");
export const filesVideo = path.join(WORK, "files.y4m");
export const segmentsVideo = path.join(WORK, "segments.y4m");
export const encryptedVideo = path.join(WORK, "encrypted.y4m");
export const webmVideo = path.join(WORK, "text.webm");
export const idsFile = path.join(WORK, "ids.json");

export interface Ids {
  /** The CLI device's ID. */
  cli: string;
  /** ID and stored secret of the identity the browser is given. */
  web: string;
  webSecret: string;
}

export const readIds = (): Ids => JSON.parse(fs.readFileSync(idsFile, "utf8"));

/** Deterministic pseudo-random bytes. */
export function noise(length: number): Buffer {
  const out = Buffer.alloc(length);
  let x = 0x9e3779b9;
  for (let i = 0; i < length; i++) {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    out[i] = x & 0xff;
  }
  return out;
}
