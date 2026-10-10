// Unit tests for launcher.js (run with `node --test npm/`). Not shipped:
// assemble.py copies only launcher.js into the npm package.
"use strict";
const test = require("node:test");
const assert = require("node:assert");
const { findBinary, launchErrorMessage } = require("./launcher.js");

const installed = (...pkgs) => (pkg, exe) => (pkgs.includes(pkg) ? `/nm/${pkg}/bin/${exe}` : null);
const linux = (musl, resolve) => ({ platform: "linux", arch: "x64", musl: () => musl, resolve });

test("glibc host uses the glibc build", () => {
  const f = findBinary(linux(false, installed("qrsend-bin-linux-x64", "qrsend-bin-linux-x64-musl")));
  assert.strictEqual(f.pkg, "qrsend-bin-linux-x64");
  assert.strictEqual(f.muslFallback, false);
});

test("musl host prefers the musl build", () => {
  const f = findBinary(linux(true, installed("qrsend-bin-linux-x64", "qrsend-bin-linux-x64-musl")));
  assert.strictEqual(f.pkg, "qrsend-bin-linux-x64-musl");
  assert.strictEqual(f.muslFallback, false);
});

test("musl host with only the glibc build is flagged as a fallback", () => {
  const f = findBinary(linux(true, installed("qrsend-bin-linux-x64")));
  assert.strictEqual(f.pkg, "qrsend-bin-linux-x64");
  assert.strictEqual(f.muslFallback, true);
});

test("missing packages and unsupported platforms are reported", () => {
  assert.match(findBinary(linux(false, installed())).error, /could not find binary package/);
  assert.match(findBinary({ platform: "aix", arch: "ppc64" }).error, /unsupported platform/);
});

test("ENOENT after a musl fallback explains the glibc/musl mismatch", () => {
  const err = Object.assign(new Error("spawnSync /nm/x ENOENT"), { code: "ENOENT" });
  const fallback = { pkg: "qrsend-bin-linux-x64", muslFallback: true };
  const msg = launchErrorMessage(fallback, err, "x64");
  assert.match(msg, /musl system/);
  assert.match(msg, /npm install -g qrsend-bin-linux-x64-musl/);
  // Without the fallback, the message stays the plain error.
  assert.strictEqual(
    launchErrorMessage({ pkg: "qrsend-bin-linux-x64", muslFallback: false }, err, "x64"),
    "qrsend: failed to launch: spawnSync /nm/x ENOENT"
  );
});

test("platforms added later resolve to scoped packages", () => {
  const f = findBinary({ platform: "win32", arch: "ia32", resolve: installed("@qrsend/cli-bin-win32-ia32") });
  assert.strictEqual(f.pkg, "@qrsend/cli-bin-win32-ia32");
  assert.match(f.path, /qrsend\.exe$/);
  const bsd = findBinary({ platform: "freebsd", arch: "x64", resolve: installed("@qrsend/cli-bin-freebsd-x64") });
  assert.strictEqual(bsd.pkg, "@qrsend/cli-bin-freebsd-x64");
});

test("32-bit ARM picks ARMv7 or ARMv6, glibc or musl", () => {
  const all = installed(
    "@qrsend/cli-bin-linux-arm",
    "@qrsend/cli-bin-linux-arm-musl",
    "@qrsend/cli-bin-linux-armv6",
    "@qrsend/cli-bin-linux-armv6-musl",
  );
  const arm = (musl, armV6, resolve = all) =>
    findBinary({ platform: "linux", arch: "arm", musl: () => musl, armV6: () => armV6, resolve }).pkg;
  assert.strictEqual(arm(false, false), "@qrsend/cli-bin-linux-arm");
  assert.strictEqual(arm(true, false), "@qrsend/cli-bin-linux-arm-musl");
  assert.strictEqual(arm(false, true), "@qrsend/cli-bin-linux-armv6");
  assert.strictEqual(arm(true, true), "@qrsend/cli-bin-linux-armv6-musl");
  // An ARMv7 host can fall back to the ARMv6 build; never the other way round.
  assert.strictEqual(arm(false, false, installed("@qrsend/cli-bin-linux-armv6")), "@qrsend/cli-bin-linux-armv6");
  assert.match(
    findBinary({ platform: "linux", arch: "arm", musl: () => false, armV6: () => true, resolve: installed("@qrsend/cli-bin-linux-arm") }).error,
    /could not find/,
  );
});

test("Ctrl-C: the launcher waits for the program and passes its exit status on", { skip: process.platform === "win32" }, async () => {
  const fs = require("fs");
  const os = require("os");
  const path = require("path");
  const { spawn } = require("child_process");
  const { PLATFORMS } = require("./launcher.js");
  const entry = PLATFORMS[`${process.platform} ${process.arch}`];
  if (!entry) return;
  // An installed launcher next to a stand-in for the program, which (like
  // `qrsend recv`) takes a moment on Ctrl-C to save and say how to go on.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "qrsend-launcher-"));
  const launcher = path.join(root, "node_modules", "qrsend-cli", "bin", "qrsend.js");
  fs.mkdirSync(path.dirname(launcher), { recursive: true });
  fs.copyFileSync(path.join(__dirname, "launcher.js"), launcher);
  for (const pkg of [entry.pkg, entry.muslPkg].filter(Boolean)) {
    const bin = path.join(root, "node_modules", ...pkg.split("/"), "bin", "qrsend");
    fs.mkdirSync(path.dirname(bin), { recursive: true });
    fs.writeFileSync(bin, "#!/bin/sh\ntrap 'sleep 0.5; echo saved; exit 2' INT\necho running\nwhile :; do sleep 0.1; done\n", { mode: 0o755 });
  }
  const child = spawn(process.execPath, [launcher, "recv"], { detached: true, stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  child.stdout.on("data", (d) => (out += d));
  await new Promise((resolve) => child.stdout.once("data", resolve));
  // What a terminal does on Ctrl-C: the signal goes to the whole group.
  process.kill(-child.pid, "SIGINT");
  // ("close": the program's output is all in as well.)
  const [code, signal] = await new Promise((resolve) => child.on("close", (c, s) => resolve([c, s])));
  fs.rmSync(root, { recursive: true, force: true });
  assert.deepStrictEqual([code, signal], [2, null]);
  assert.match(out, /saved/);
});
