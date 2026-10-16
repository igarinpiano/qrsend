// Encoding and decoding in Node, and both against the QRSend CLI.
// Run after `npm run build`. The CLI tests use target/debug/qrsend (or
// QRSEND_BIN) and are skipped without it.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { Decoder, Encoder, QR_CODE_TRADEMARK, init, version } from "../dist/index.js";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");

function noise(n, seed = 1) {
  const out = new Uint8Array(n);
  let x = seed * 0x9e3779b9 || 1;
  for (let i = 0; i < n; i++) {
    x ^= x << 13;
    x ^= x >>> 17;
    x ^= x << 5;
    out[i] = x & 0xff;
  }
  return out;
}

/** Codes from `encoder`: `rounds` rounds of them. */
function codes(encoder, rounds = 2) {
  const out = [];
  for (let i = 0; i < encoder.codesPerRound * rounds; i++) out.push(encoder.next());
  return out;
}

async function decode(texts) {
  const decoder = await Decoder.create();
  for (const t of texts) {
    decoder.push(t);
    if (decoder.complete) break;
  }
  assert.ok(decoder.complete, `not complete: ${JSON.stringify(decoder.progress)}`);
  return decoder;
}

test("loads and tells its version", async () => {
  await init();
  assert.match(await version(), /^\d+\.\d+\.\d+$/);
  assert.match(QR_CODE_TRADEMARK, /DENSO WAVE INCORPORATED/);
});

test("a text comes through", async () => {
  const encoder = await Encoder.create("Hello, QR codes! 日本語も。");
  const decoder = await decode(codes(encoder));
  assert.deepEqual(decoder.result(), { kind: "text", text: "Hello, QR codes! 日本語も。" });
  assert.equal(decoder.progress.state, "complete");
  assert.equal(decoder.progress.ratio, 1);
});

test("files and folders come through, with codes lost and out of order", async () => {
  const big = noise(400_000);
  const encoder = await Encoder.create([
    { path: "photos/2024/a.bin", data: big, lastModified: 1_700_000_000_000 },
    { path: "photos/empty" },
    { path: "notes.txt", data: "hello" },
    { path: "blob.dat", data: new Blob([new Uint8Array([1, 2, 3])]) },
  ]);
  const texts = codes(encoder, 3).filter((_, i) => i % 3 !== 1).reverse();
  const decoder = await decode(texts);
  const r = decoder.result();
  assert.equal(r.kind, "files");
  const byPath = Object.fromEntries(r.files.map((f) => [f.path, f]));
  assert.deepEqual(Object.keys(byPath).sort(), ["blob.dat", "notes.txt", "photos/2024/a.bin"]);
  assert.deepEqual(new Uint8Array(await byPath["photos/2024/a.bin"].data.arrayBuffer()), big);
  assert.equal(byPath["photos/2024/a.bin"].name, "a.bin");
  assert.equal(byPath["photos/2024/a.bin"].lastModified, 1_700_000_000_000);
  assert.equal(await byPath["notes.txt"].data.text(), "hello");
  assert.deepEqual([...new Uint8Array(await byPath["blob.dat"].data.arrayBuffer())], [1, 2, 3]);
  assert.ok(r.directories.includes("photos/empty"));
  assert.deepEqual(
    decoder.entries().filter((e) => !e.directory).map((e) => e.path).sort(),
    ["blob.dat", "notes.txt", "photos/2024/a.bin"],
  );
});

test("a Blob or bytes become one file", async () => {
  const one = await decode(codes(await Encoder.create(new Uint8Array([9, 8, 7]), { name: "x.bin" })));
  const r = one.result();
  assert.equal(r.kind, "files");
  assert.equal(r.files[0].path, "x.bin");
  const file = new File(["content"], "named.txt", { lastModified: 1_600_000_000_000 });
  const two = (await decode(codes(await Encoder.create(file)))).result();
  assert.equal(two.files[0].path, "named.txt");
  assert.equal(two.files[0].lastModified, 1_600_000_000_000);
});

test("paths are made safe to save", async () => {
  // What could land outside the receiver's folder is not sent at all.
  await assert.rejects(Encoder.create([{ path: "../../etc/passwd", data: "no" }]), /unsafe path/);
  await assert.rejects(Encoder.create([{ path: "C:/Windows/x", data: "no" }]), /unsafe path/);
  // Names some systems refuse, and names that differ only in case, are changed.
  const encoder = await Encoder.create([
    { path: "CON", data: "1" },
    { path: "a?.txt", data: "2" },
    { path: "Same.txt", data: "3" },
    { path: "same.txt", data: "4" },
  ]);
  const paths = (await decode(codes(encoder))).result().files.map((f) => f.path);
  assert.equal(new Set(paths.map((p) => p.toLowerCase())).size, 4, paths.join(", "));
  assert.ok(!paths.includes("CON"), paths.join(", "));
  assert.ok(!paths.includes("a?.txt"), paths.join(", "));
});

test("the same content is the same transfer", async () => {
  const a = await Encoder.create([{ path: "a", data: "same", lastModified: 1 }]);
  const b = await Encoder.create([{ path: "a", data: "same", lastModified: 1 }]);
  const c = await Encoder.create([{ path: "a", data: "other", lastModified: 1 }]);
  assert.equal(a.session, b.session);
  assert.notEqual(a.session, c.session);
  // Half from one, half from the other: one transfer.
  const half = (e) => codes(e, 2).filter((_, i) => i % 2 === 0);
  const decoder = await decode([...half(a).slice(0, 1), ...half(b)]);
  assert.equal((await decoder.result().files[0].data.text()), "same");
});

test("codes have the size asked for", async () => {
  const encoder = await Encoder.create("x".repeat(3000), { qrVersion: 12, errorCorrection: "L" });
  assert.equal(encoder.qrVersion, 12);
  assert.equal(encoder.errorCorrection, "L");
  assert.equal(encoder.size, 12 * 4 + 17);
  const m = encoder.nextModules();
  assert.equal(m.length, encoder.size * encoder.size);
  assert.ok(m.every((v) => v === 0 || v === 1));
  assert.equal(encoder.count, 1);
  // A small transfer picks small codes by itself.
  assert.ok((await Encoder.create("hi")).qrVersion <= 10);
});

test("bad options and content are refused", async () => {
  await assert.rejects(Encoder.create("x", { qrVersion: 41 }), /1 to 40/);
  await assert.rejects(Encoder.create("x", { qrVersion: 10, errorCorrection: "Z" }));
  await assert.rejects(Encoder.create([{ path: "", data: "x" }]), /path/);
});

test("other texts and other transfers are passed over", async () => {
  const decoder = await Decoder.create();
  assert.throws(() => decoder.result(), /not complete/);
  assert.equal(decoder.push(["https://example.com", "hello", ""]), false);
  assert.equal(decoder.progress.state, "waiting");
  const one = await Encoder.create("one");
  const two = await Encoder.create("two");
  decoder.push(one.next());
  assert.equal(decoder.progress.state, "receiving");
  decoder.push(codes(two));
  assert.equal(decoder.progress.otherTransfer, true);
  decoder.push(codes(one));
  assert.equal(decoder.result().text, "one");
});

test("progress grows as codes come", async () => {
  const encoder = await Encoder.create(noise(100_000), { compress: false });
  const decoder = await Decoder.create();
  let last = 0;
  for (const t of codes(encoder)) {
    if (decoder.push(t)) {
      const p = decoder.progress;
      assert.ok(p.ratio >= last - 1e-9, `${p.ratio} after ${last}`);
      last = p.ratio;
    }
    if (decoder.complete) break;
  }
  assert.equal(last, 1);
  assert.ok(encoder.bytes > 100_000);
});

// ---------------------------------------------------------------- the CLI

const cli = process.env.QRSEND_BIN ?? path.join(root, "target/debug", process.platform === "win32" ? "qrsend.exe" : "qrsend");
const noCli = existsSync(cli) ? false : `no CLI at ${cli} (cargo build -p qrsend)`;

function run(args, env = {}) {
  const r = spawnSync(cli, args, { encoding: "utf8", env: { ...process.env, ...env } });
  if (r.status !== 0) throw new Error(`qrsend ${args.join(" ")}: ${r.status}\n${r.stderr}`);
  return r;
}

function sandbox() {
  const dir = mkdtempSync(path.join(tmpdir(), "qrsend-web-"));
  return {
    dir,
    env: {
      QRSEND_DATA_DIR: path.join(dir, "data"),
      QRSEND_CACHE_DIR: path.join(dir, "cache"),
      QRSEND_CONFIG_DIR: path.join(dir, "config"),
      QRSEND_LOG: "off",
    },
  };
}

test("reads what the CLI sends", { skip: noCli }, async () => {
  const { dir, env } = sandbox();
  mkdirSync(path.join(dir, "in/sub"), { recursive: true });
  const big = noise(200_000, 7);
  writeFileSync(path.join(dir, "in/sub/big.bin"), big);
  writeFileSync(path.join(dir, "in/hello.txt"), "hello from the CLI");
  const out = path.join(dir, "codes.txt");
  run(["send", path.join(dir, "in"), "--plain", "--export-text", out], env);
  const lines = readFileSync(out, "utf8").split("\n").filter(Boolean);
  const r = (await decode(lines)).result();
  assert.equal(r.kind, "files");
  const byPath = Object.fromEntries(r.files.map((f) => [f.path, f]));
  assert.equal(await byPath["in/hello.txt"].data.text(), "hello from the CLI");
  assert.deepEqual(new Uint8Array(await byPath["in/sub/big.bin"].data.arrayBuffer()), big);
});

test("the CLI reads what this sends", { skip: noCli }, async () => {
  const { dir, env } = sandbox();
  const big = noise(150_000, 3);
  const encoder = await Encoder.create([
    { path: "folder/big.bin", data: big },
    { path: "folder/note.txt", data: "hello from the library" },
  ]);
  writeFileSync(path.join(dir, "codes.txt"), codes(encoder, 2).join("\n") + "\n");
  run(["recv", "--text", path.join(dir, "codes.txt"), "-o", path.join(dir, "out")], env);
  assert.deepEqual(readdirSync(path.join(dir, "out/folder")).sort(), ["big.bin", "note.txt"]);
  assert.deepEqual(new Uint8Array(readFileSync(path.join(dir, "out/folder/big.bin"))), big);
  assert.equal(readFileSync(path.join(dir, "out/folder/note.txt"), "utf8"), "hello from the library");
});

test("an encrypted transfer from the CLI is passed over, and said so", { skip: noCli }, async () => {
  const { dir, env } = sandbox();
  const id = run(["id", "--no-qr", "--name", "test"], env)
    .stdout.split(/\s+/)
    .find((w) => w.startsWith("qrsend-id:"));
  assert.ok(id, "an ID");
  writeFileSync(path.join(dir, "secret.txt"), "secret");
  const out = path.join(dir, "codes.txt");
  run(["send", path.join(dir, "secret.txt"), "--to", id, "--export-text", out], env);
  const decoder = await Decoder.create();
  decoder.push(readFileSync(out, "utf8").split("\n").filter(Boolean));
  const p = decoder.progress;
  assert.equal(p.encrypted, true);
  assert.equal(p.state, "waiting");
  assert.equal(decoder.complete, false);
});
