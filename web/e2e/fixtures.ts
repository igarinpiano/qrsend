// Shared helpers: the qrsend CLI binary and an isolated environment for it.
import { execFileSync, type ExecFileSyncOptions } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const BIN = process.env.QRSEND_BIN ?? path.resolve(import.meta.dirname, "../../target/release/qrsend");
export const WORK = path.join(os.tmpdir(), "qrsend-e2e");

export function cli(args: string[], opts: ExecFileSyncOptions = {}): string {
  return execFileSync(BIN, args, {
    cwd: WORK,
    env: {
      ...process.env,
      QRSEND_DATA_DIR: path.join(WORK, "data"),
      QRSEND_CACHE_DIR: path.join(WORK, "cache"),
      QRSEND_CONFIG_DIR: path.join(WORK, "config"),
    },
    encoding: "utf8",
    ...opts,
  }) as string;
}

export const hasCli = fs.existsSync(BIN);

export const FAKE_TEXT = "Hello from the CLI 👋 — QRSend e2e";
export const textVideo = path.join(WORK, "text.y4m");
export const filesVideo = path.join(WORK, "files.y4m");
