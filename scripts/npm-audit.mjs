#!/usr/bin/env node
// `npm audit`, with the same rule as deny.toml: every advisory fails the run,
// whatever its severity, unless it is listed below with the reason. Run it in
// the directory of the package (web/ or packages/qrsend-web/), after `npm ci`.
//
// npm has no way to ignore one advisory, and some have no fixed release to
// move to; this is for those only. A listed advisory that no longer turns up
// is reported, so that its entry gets removed.

import { spawnSync } from "node:child_process";
import { relative } from "node:path";
import { fileURLToPath } from "node:url";

// By directory (from the root of the repository): each has its own lockfile.
const IGNORED_IN = {
  web: {
    // deepmerge (every release from 4.2.0, the latest being 4.3.1, which has
    // not changed since 2023): mergeObject() lets a "__proto__" key of the
    // input reach the prototype of the returned object. No fixed release.
    // Only build tools use it, to merge their own options:
    // @sveltejs/vite-plugin-svelte (every version) and @rollup/plugin-node-resolve
    // (through workbox-build, for the service worker). Neither is given
    // anything that comes from outside the repository, and none of it is in
    // what is deployed. Remove once they drop it or it is fixed.
    "GHSA-gcqx-w48m-w2h4":
      "deepmerge: no fixed release; build-time only, merging our own options",
  },
  "packages/qrsend-web": {},
};

const root = fileURLToPath(new URL("..", import.meta.url));
const here = relative(root, process.cwd()).split("\\").join("/");
if (!(here in IGNORED_IN)) {
  console.error(`Run this in one of: ${Object.keys(IGNORED_IN).join(", ")} (not "${here}").`);
  process.exit(1);
}
const IGNORED = IGNORED_IN[here];

const run = spawnSync("npm", ["audit", "--json"], {
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
  shell: process.platform === "win32",
});
let report;
try {
  report = JSON.parse(run.stdout);
} catch {
  process.stderr.write(run.stdout + run.stderr);
  console.error("npm audit did not give a report.");
  process.exit(1);
}
if (report.error) {
  console.error(`npm audit failed: ${report.error.summary ?? JSON.stringify(report.error)}`);
  process.exit(1);
}

// The advisories themselves; packages that only depend on them name them by
// package name in `via`, and are covered by them.
const found = new Map();
for (const [name, entry] of Object.entries(report.vulnerabilities ?? {})) {
  for (const via of entry.via ?? []) {
    if (typeof via !== "object") continue;
    const id = String(via.url ?? "").match(/GHSA(-[0-9a-z]{4}){3}/)?.[0] ?? `npm-${via.source}`;
    const seen = found.get(id) ?? { title: via.title, severity: via.severity, packages: new Set() };
    seen.packages.add(`${via.name ?? name} ${via.range ?? ""}`.trim());
    found.set(id, seen);
  }
}

let failed = false;
for (const [id, { title, severity, packages }] of found) {
  const which = [...packages].join(", ");
  if (id in IGNORED) {
    console.log(`ignored  ${id} (${severity}) ${which}: ${title}\n         because: ${IGNORED[id]}`);
  } else {
    console.log(`FOUND    ${id} (${severity}) ${which}: ${title}`);
    failed = true;
  }
}
for (const id of Object.keys(IGNORED)) {
  if (!found.has(id)) {
    const message = `${id} is ignored for ${here} in scripts/npm-audit.mjs but no longer found; remove it.`;
    console.log(process.env.GITHUB_ACTIONS ? `::warning::${message}` : `note: ${message}`);
  }
}
if (failed) {
  console.error("\nAdvisories above: update the packages (npm audit fix, overrides), or, if there is no fixed release, list them in scripts/npm-audit.mjs with the reason.");
  process.exit(1);
}
console.log(found.size ? "No advisories other than the ignored ones." : "No advisories.");
