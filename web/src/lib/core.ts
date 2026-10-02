// Loads the Rust core (WebAssembly) once.
import init from "../wasm/qrsend_wasm.js";
import wasmUrl from "../wasm/qrsend_wasm_bg.wasm?url";

export * from "../wasm/qrsend_wasm.js";

let loading: Promise<unknown> | undefined;

export function ready(): Promise<unknown> {
  loading ??= init({ module_or_path: wasmUrl });
  return loading;
}
