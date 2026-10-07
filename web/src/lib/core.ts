// Loads the Rust core (WebAssembly) once.
import init from "../wasm/qrsend_wasm.js";
import wasmUrl from "../wasm/qrsend_wasm_bg.wasm?url";

export * from "../wasm/qrsend_wasm.js";

let loading: Promise<unknown> | undefined;

/** Tries a few times: networks hiccup. (Used on pages and in workers alike.) */
async function load(): Promise<unknown> {
  const pauses = [400, 1500];
  for (let attempt = 0; ; attempt++) {
    try {
      return await init({ module_or_path: wasmUrl });
    } catch (e) {
      // A file that is not there will not appear by asking again.
      if (attempt >= pauses.length || /\b404\b/.test(String(e))) throw e;
      await new Promise((r) => setTimeout(r, pauses[attempt]));
    }
  }
}

export function ready(): Promise<unknown> {
  loading ??= load().catch((e) => {
    // Not remembered: the next call tries afresh.
    loading = undefined;
    throw e;
  });
  return loading;
}
