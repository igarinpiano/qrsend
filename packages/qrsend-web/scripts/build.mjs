// Builds the package: the WebAssembly core (crates/qrsend-js) with its
// JavaScript bindings, then the TypeScript, into dist/.
import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, rmSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(here, "../..");
const run = (cmd, args, cwd = root) => execFileSync(cmd, args, { cwd, stdio: "inherit", shell: process.platform === "win32" });

run("cargo", ["build", "-p", "qrsend-js", "--target", "wasm32-unknown-unknown", "--profile", "wasm-release"]);
rmSync(path.join(here, "src/wasm"), { recursive: true, force: true });
run("wasm-bindgen", [
  "--target", "web",
  "--out-dir", "packages/qrsend-web/src/wasm",
  "target/wasm32-unknown-unknown/wasm-release/qrsend_js.wasm",
]);
rmSync(path.join(here, "dist"), { recursive: true, force: true });
run("npx", ["tsc", "-p", "."], here);
// tsc compiles the TypeScript only; the generated bindings go as they are.
mkdirSync(path.join(here, "dist/wasm"), { recursive: true });
cpSync(path.join(here, "src/wasm"), path.join(here, "dist/wasm"), { recursive: true });
rmSync(path.join(here, "dist/wasm/.gitignore"), { force: true });
cpSync(path.join(root, "LICENSE"), path.join(here, "LICENSE"));
