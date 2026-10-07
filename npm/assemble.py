#!/usr/bin/env python3
"""Assembles the npm packages from release binaries (adapted from dirlens).

Input:  --version X.Y.Z --binaries <dir>, with <dir>/<target>/qrsend(.exe)
Output: --out <dir> with the main package qrsend-cli/ (a small Node launcher;
        npm rejects the unscoped name "qrsend" as too similar to send/resend) and
        one qrsend-bin-<platform> package per target, installed through
        optionalDependencies — the approach used by esbuild, swc and Biome.
"""
import argparse
import json
import os
import shutil
import stat

# target -> (package, os, cpu, libc or None, executable)
# os/cpu are Node's process.platform / process.arch. libc separates the glibc
# and musl builds of one architecture (npm 9+); launcher.js also detects musl
# and, for 32-bit ARM, ARMv6 vs ARMv7 (npm cannot tell those apart).
#
# The first eight packages keep their original unscoped names. Every package
# added later lives in the @qrsend scope: npm's spam filter rejects new
# unscoped "*-win32-*" names, and scoped names avoid that class of problem.
def scoped(platform):
    return f"@qrsend/cli-bin-{platform}"


TARGETS = {
    "aarch64-apple-darwin": ("qrsend-bin-darwin-arm64", ["darwin"], ["arm64"], None, "qrsend"),
    "x86_64-apple-darwin": ("qrsend-bin-darwin-x64", ["darwin"], ["x64"], None, "qrsend"),
    "aarch64-unknown-linux-gnu": ("qrsend-bin-linux-arm64", ["linux"], ["arm64"], None, "qrsend"),
    "x86_64-unknown-linux-gnu": ("qrsend-bin-linux-x64", ["linux"], ["x64"], None, "qrsend"),
    "aarch64-unknown-linux-musl": ("qrsend-bin-linux-arm64-musl", ["linux"], ["arm64"], ["musl"], "qrsend"),
    "x86_64-unknown-linux-musl": ("qrsend-bin-linux-x64-musl", ["linux"], ["x64"], ["musl"], "qrsend"),
    "x86_64-pc-windows-msvc": ("qrsend-bin-win32-x64", ["win32"], ["x64"], None, "qrsend.exe"),
    "aarch64-pc-windows-msvc": ("qrsend-bin-win32-arm64", ["win32"], ["arm64"], None, "qrsend.exe"),
    # ── added in 0.1.1 (scoped) ──
    "i686-pc-windows-msvc": (scoped("win32-ia32"), ["win32"], ["ia32"], None, "qrsend.exe"),
    "i686-unknown-linux-gnu": (scoped("linux-ia32"), ["linux"], ["ia32"], None, "qrsend"),
    "i686-unknown-linux-musl": (scoped("linux-ia32-musl"), ["linux"], ["ia32"], ["musl"], "qrsend"),
    "armv7-unknown-linux-gnueabihf": (scoped("linux-arm"), ["linux"], ["arm"], None, "qrsend"),
    "armv7-unknown-linux-musleabihf": (scoped("linux-arm-musl"), ["linux"], ["arm"], ["musl"], "qrsend"),
    "arm-unknown-linux-gnueabihf": (scoped("linux-armv6"), ["linux"], ["arm"], None, "qrsend"),
    "arm-unknown-linux-musleabihf": (scoped("linux-armv6-musl"), ["linux"], ["arm"], ["musl"], "qrsend"),
    "riscv64gc-unknown-linux-gnu": (scoped("linux-riscv64"), ["linux"], ["riscv64"], None, "qrsend"),
    "powerpc64le-unknown-linux-gnu": (scoped("linux-ppc64"), ["linux"], ["ppc64"], None, "qrsend"),
    "s390x-unknown-linux-gnu": (scoped("linux-s390x"), ["linux"], ["s390x"], None, "qrsend"),
    "loongarch64-unknown-linux-gnu": (scoped("linux-loong64"), ["linux"], ["loong64"], None, "qrsend"),
    "aarch64-linux-android": (scoped("android-arm64"), ["android"], ["arm64"], None, "qrsend"),
    "armv7-linux-androideabi": (scoped("android-arm"), ["android"], ["arm"], None, "qrsend"),
    "x86_64-linux-android": (scoped("android-x64"), ["android"], ["x64"], None, "qrsend"),
    "i686-linux-android": (scoped("android-ia32"), ["android"], ["ia32"], None, "qrsend"),
    "x86_64-unknown-freebsd": (scoped("freebsd-x64"), ["freebsd"], ["x64"], None, "qrsend"),
    "x86_64-unknown-netbsd": (scoped("netbsd-x64"), ["netbsd"], ["x64"], None, "qrsend"),
    "x86_64-unknown-illumos": (scoped("sunos-x64"), ["sunos"], ["x64"], None, "qrsend"),
}

MAIN = "qrsend-cli"  # the command it installs is still `qrsend`
DESCRIPTION = "Send any data — text, files, folders — through a stream of QR codes"
REPO = {"type": "git", "url": "git+https://github.com/igarinpiano/qrsend.git"}
HOMEPAGE = "https://igarinpiano.github.io/qrsend/"


def write_json(path, obj):
    with open(path, "w", encoding="utf-8") as f:
        json.dump(obj, f, ensure_ascii=False, indent=2)
        f.write("\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--binaries", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--docs", help="directory holding README.md and LICENSE (default: repository root)")
    args = ap.parse_args()
    here = os.path.dirname(os.path.abspath(__file__))
    repo_root = args.docs or os.path.dirname(here)
    os.makedirs(args.out, exist_ok=True)

    optional = {}
    for target, (pkg, os_list, cpu_list, libc_list, exe) in TARGETS.items():
        src = os.path.join(args.binaries, target, exe)
        if not os.path.isfile(src):
            print(f"skip {target} (no binary)")
            continue
        pdir = os.path.join(args.out, *pkg.split("/"))
        os.makedirs(os.path.join(pdir, "bin"), exist_ok=True)
        dst = os.path.join(pdir, "bin", exe)
        shutil.copy2(src, dst)
        os.chmod(dst, os.stat(dst).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
        pkg_json = {
            "name": pkg,
            "version": args.version,
            "description": f"qrsend binary for {target}",
            "repository": REPO,
            "homepage": HOMEPAGE,
            "license": "Apache-2.0",
            "os": os_list,
            "cpu": cpu_list,
            "files": ["bin/"],
        }
        if libc_list:
            pkg_json["libc"] = libc_list
        write_json(os.path.join(pdir, "package.json"), pkg_json)
        optional[pkg] = args.version

    if not optional:
        raise SystemExit("no binaries found")

    main_dir = os.path.join(args.out, MAIN)
    os.makedirs(os.path.join(main_dir, "bin"), exist_ok=True)
    shutil.copy2(os.path.join(here, "launcher.js"), os.path.join(main_dir, "bin", "qrsend.js"))
    for doc in ["README.md", "LICENSE"]:
        shutil.copy2(os.path.join(repo_root, doc), os.path.join(main_dir, doc))
    write_json(os.path.join(main_dir, "package.json"), {
        "name": MAIN,
        "version": args.version,
        "description": DESCRIPTION,
        "keywords": ["qr", "qrcode", "file-transfer", "air-gap", "raptorq", "cli"],
        "repository": REPO,
        "homepage": HOMEPAGE,
        "license": "Apache-2.0",
        "bin": {"qrsend": "bin/qrsend.js"},
        "files": ["bin/", "README.md", "LICENSE"],
        "optionalDependencies": optional,
    })
    print(f"assembled: {MAIN} + {len(optional)} platform packages -> {args.out}")


if __name__ == "__main__":
    main()
