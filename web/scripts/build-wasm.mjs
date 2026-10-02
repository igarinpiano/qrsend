// Builds crates/qrsend-wasm and generates JS bindings into src/wasm/.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const run = (cmd, args) => execFileSync(cmd, args, { cwd: root, stdio: "inherit" });

run("cargo", ["build", "-p", "qrsend-wasm", "--target", "wasm32-unknown-unknown", "--profile", "wasm-release"]);
run("wasm-bindgen", [
  "--target", "web",
  "--out-dir", "web/src/wasm",
  "target/wasm32-unknown-unknown/wasm-release/qrsend_wasm.wasm",
]);
