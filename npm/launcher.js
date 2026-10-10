#!/usr/bin/env node
// qrsend – thin launcher for npm distribution.
// Finds and execs the real binary from the platform-specific binary package
// installed alongside via optionalDependencies (the same well-established
// approach used by esbuild / swc / Biome / turbo).
"use strict";
const { spawn } = require("child_process");
const path = require("path");
const fs = require("fs");

// The first eight platform packages are unscoped (qrsend-bin-*); platforms
// added later live in the @qrsend scope.
const s = (platform) => `@qrsend/cli-bin-${platform}`;

// pkg: the default (glibc on Linux) build. muslPkg: the static musl build,
// preferred on musl hosts (Alpine, etc.). On 32-bit ARM, v6Pkg / v6MuslPkg are
// the ARMv6 builds (Raspberry Pi 1 / Zero); os/cpu alone cannot tell ARMv6
// from ARMv7, or glibc from musl, so several packages may be installed and
// the right one is picked here at run time.
const PLATFORMS = {
  "darwin arm64": { pkg: "qrsend-bin-darwin-arm64" },
  "darwin x64": { pkg: "qrsend-bin-darwin-x64" },
  "linux arm64": { pkg: "qrsend-bin-linux-arm64", muslPkg: "qrsend-bin-linux-arm64-musl" },
  "linux x64": { pkg: "qrsend-bin-linux-x64", muslPkg: "qrsend-bin-linux-x64-musl" },
  "linux ia32": { pkg: s("linux-ia32"), muslPkg: s("linux-ia32-musl") },
  "linux arm": {
    pkg: s("linux-arm"),
    muslPkg: s("linux-arm-musl"),
    v6Pkg: s("linux-armv6"),
    v6MuslPkg: s("linux-armv6-musl"),
  },
  "linux riscv64": { pkg: s("linux-riscv64") },
  "linux ppc64": { pkg: s("linux-ppc64") },
  "linux s390x": { pkg: s("linux-s390x") },
  "linux loong64": { pkg: s("linux-loong64") },
  "android arm64": { pkg: s("android-arm64") },
  "android arm": { pkg: s("android-arm") },
  "android x64": { pkg: s("android-x64") },
  "android ia32": { pkg: s("android-ia32") },
  "freebsd x64": { pkg: s("freebsd-x64") },
  "netbsd x64": { pkg: s("netbsd-x64") },
  "sunos x64": { pkg: s("sunos-x64") },
  "win32 x64": { pkg: "qrsend-bin-win32-x64" },
  "win32 arm64": { pkg: "qrsend-bin-win32-arm64" },
  "win32 ia32": { pkg: s("win32-ia32") },
};

// Standard detection method used by esbuild and others: Node's process.report
// includes the glibc version it was built against (absent for musl builds of
// Node). On older Node versions where process.report isn't available, fall
// back to checking whether ldd's output contains "musl".
function isMusl() {
  if (process.platform !== "linux") return false;
  if (!process.report || typeof process.report.getReport !== "function") {
    try {
      return fs.readFileSync("/usr/bin/ldd", "utf8").includes("musl");
    } catch (e) {
      return false;
    }
  }
  const { glibcVersionRuntime } = process.report.getReport().header;
  return !glibcVersionRuntime;
}

// Node records the ARM version it was built for; ARMv6 hosts cannot run the
// ARMv7 build.
function isArmV6() {
  return process.arch === "arm" && String(process.config.variables.arm_version) === "6";
}

function resolveFromPkg(pkg, exe) {
  try {
    return require.resolve(`${pkg}/bin/${exe}`);
  } catch (e) {
    // Fallback from node_modules/qrsend-cli/bin/ to node_modules/<pkg>/bin/
    const local = path.join(__dirname, "..", "..", ...pkg.split("/"), "bin", exe);
    if (fs.existsSync(local)) return local;
    return null;
  }
}

// Packages to try, best first.
function candidatesFor(entry, onMusl, armV6) {
  const glibc = armV6 ? [entry.v6Pkg] : [entry.pkg, entry.v6Pkg];
  const musl = armV6 ? [entry.v6MuslPkg] : [entry.muslPkg, entry.v6MuslPkg];
  return (onMusl ? [...musl, ...glibc] : glibc).filter(Boolean);
}

// Returns { path, pkg, muslFallback } for the binary to run, or { error }.
// muslFallback is true when the host is musl but only a glibc build was
// found (e.g. npm < 9 installed it, or the musl package was omitted). That
// glibc binary usually can't start on musl (no glibc dynamic loader), and the
// spawn then fails with a bare ENOENT, so the caller explains it instead.
function findBinary({
  platform = process.platform,
  arch = process.arch,
  musl = isMusl,
  armV6 = isArmV6,
  resolve = resolveFromPkg,
} = {}) {
  const key = `${platform} ${arch}`;
  const entry = PLATFORMS[key];
  if (!entry) return { error: `qrsend: unsupported platform (${key})` };
  const exe = platform === "win32" ? "qrsend.exe" : "qrsend";
  const onMusl = Boolean(entry.muslPkg) && musl();
  const candidates = candidatesFor(entry, onMusl, arch === "arm" && armV6());
  const muslPkgs = [entry.muslPkg, entry.v6MuslPkg].filter(Boolean);
  for (const pkg of candidates) {
    const resolved = resolve(pkg, exe);
    if (resolved) return { path: resolved, pkg, muslFallback: onMusl && !muslPkgs.includes(pkg) };
  }
  return {
    error:
      `qrsend: could not find binary package ${candidates.join(" / ")}.\n` +
      "Try reinstalling: npm install -g qrsend-cli (without --omit=optional / --no-optional).",
  };
}

// Message for a failed spawn. A glibc build started on a musl host fails with
// ENOENT (its ELF interpreter is missing), which on its own looks like the
// binary itself is missing.
function launchErrorMessage(found, error, arch = process.arch) {
  let msg = `qrsend: failed to launch: ${error.message}`;
  if (found.muslFallback && error.code === "ENOENT") {
    const muslPkg = PLATFORMS[`linux ${arch}`] && PLATFORMS[`linux ${arch}`].muslPkg;
    msg +=
      `\nqrsend: this looks like a musl system (Alpine, etc.), but only the glibc build (${found.pkg}) is installed,` +
      " and it cannot run without glibc." +
      `\nInstall the musl build: npm install -g ${muslPkg}` +
      " (or reinstall qrsend-cli with npm 9+ without --omit=optional / --no-optional).";
  }
  return msg;
}

// Signals the program should get too when they are sent to this process
// alone (a terminal's Ctrl-C reaches both anyway).
const FORWARDED = process.platform === "win32" ? [] : ["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"];

function main() {
  const found = findBinary();
  if (found.error) {
    console.error(found.error);
    process.exit(1);
  }
  // Waits for the program instead of blocking on it: on Ctrl-C, `qrsend
  // recv` saves what it has and prints how to continue, which takes a
  // moment, and this process must neither end before that (the shell would
  // be back while it still writes) nor lose its exit status.
  const child = spawn(found.path, process.argv.slice(2), { stdio: "inherit" });
  const forward = {};
  for (const signal of [...FORWARDED, "SIGINT"]) {
    forward[signal] = () => {
      if (FORWARDED.includes(signal)) {
        try {
          child.kill(signal);
        } catch (e) {
          /* already gone */
        }
      }
    };
    process.on(signal, forward[signal]);
  }
  child.on("error", (error) => {
    console.error(launchErrorMessage(found, error));
    process.exit(1);
  });
  child.on("exit", (code, signal) => {
    if (signal) {
      // Ended by a signal: end the same way, so that the shell sees it.
      for (const [name, handler] of Object.entries(forward)) process.removeListener(name, handler);
      process.kill(process.pid, signal);
      return;
    }
    process.exit(code === null ? 1 : code);
  });
}

if (require.main === module) {
  main();
}

module.exports = { PLATFORMS, findBinary, launchErrorMessage };
