#!/usr/bin/env python3
"""Assembles the npm packages from release binaries (adapted from dirlens).

Input:  --version X.Y.Z --binaries <dir>, with <dir>/<target>/qrsend(.exe)
Output: --out <dir> with the main package qrsend/ (a small Node launcher) and
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
# and musl builds for linux x64/arm64 (npm 9+); launcher.js also detects musl.
TARGETS = {
    "aarch64-apple-darwin": ("qrsend-bin-darwin-arm64", ["darwin"], ["arm64"], None, "qrsend"),
    "x86_64-apple-darwin": ("qrsend-bin-darwin-x64", ["darwin"], ["x64"], None, "qrsend"),
    "aarch64-unknown-linux-gnu": ("qrsend-bin-linux-arm64", ["linux"], ["arm64"], None, "qrsend"),
    "x86_64-unknown-linux-gnu": ("qrsend-bin-linux-x64", ["linux"], ["x64"], None, "qrsend"),
    "aarch64-unknown-linux-musl": ("qrsend-bin-linux-arm64-musl", ["linux"], ["arm64"], ["musl"], "qrsend"),
    "x86_64-unknown-linux-musl": ("qrsend-bin-linux-x64-musl", ["linux"], ["x64"], ["musl"], "qrsend"),
    "x86_64-pc-windows-msvc": ("qrsend-bin-win32-x64", ["win32"], ["x64"], None, "qrsend.exe"),
    "aarch64-pc-windows-msvc": ("qrsend-bin-win32-arm64", ["win32"], ["arm64"], None, "qrsend.exe"),
}

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
    args = ap.parse_args()
    here = os.path.dirname(os.path.abspath(__file__))
    repo_root = os.path.dirname(here)
    os.makedirs(args.out, exist_ok=True)

    optional = {}
    for target, (pkg, os_list, cpu_list, libc_list, exe) in TARGETS.items():
        src = os.path.join(args.binaries, target, exe)
        if not os.path.isfile(src):
            print(f"skip {target} (no binary)")
            continue
        pdir = os.path.join(args.out, pkg)
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

    main_dir = os.path.join(args.out, "qrsend")
    os.makedirs(os.path.join(main_dir, "bin"), exist_ok=True)
    shutil.copy2(os.path.join(here, "launcher.js"), os.path.join(main_dir, "bin", "qrsend.js"))
    for doc in ["README.md", "LICENSE"]:
        shutil.copy2(os.path.join(repo_root, doc), os.path.join(main_dir, doc))
    write_json(os.path.join(main_dir, "package.json"), {
        "name": "qrsend",
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
    print(f"assembled: qrsend + {len(optional)} platform packages -> {args.out}")


if __name__ == "__main__":
    main()
