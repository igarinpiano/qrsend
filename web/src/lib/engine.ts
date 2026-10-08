// Page-side handle to the engine worker (packing, frame generation, receive
// state, storage and extraction all run there, off the UI thread).
import EngineWorker from "./engine.worker?worker";
import type { EngineApi, EngineEvent } from "./engine-types";
import { log } from "./log";
import { RELOAD_HINT, recoverFromLoadFailure } from "./update";

/** Errors that mean a file of the app could not be fetched (rather than something about the transfer). */
const LOAD_FAILURE = /failed to fetch|fetching|importing a module|load failed|networkerror|engine could not be started/i;

/** If a new version went live, reloads into it; otherwise explains what to do. */
async function loadFailure(e: Error): Promise<Error> {
  await recoverFromLoadFailure();
  return new Error(`A part of the app could not be loaded (${e.message}). ${RELOAD_HINT}`);
}

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

let worker: Worker | undefined;
let nextId = 1;
const pending = new Map<number, Pending>();
const listeners = new Set<(e: EngineEvent) => void>();

function start(): Worker {
  const w = new EngineWorker();
  w.onmessage = (e: MessageEvent) => {
    const msg = e.data;
    if (msg.event === "log") {
      log(msg.area, msg.what, msg.data);
      return;
    }
    if (msg.event) {
      listeners.forEach((l) => l(msg as EngineEvent));
      return;
    }
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if (msg.error !== undefined) {
      log("engine", "failed", { message: String(msg.error).slice(0, 200) });
      const err = new Error(msg.error);
      if (LOAD_FAILURE.test(msg.error)) loadFailure(err).then(p.reject);
      else p.reject(err);
    } else p.resolve(msg.result);
  };
  w.onerror = (e) => {
    // Most often the worker's own script could not be fetched. Start a new
    // worker on the next call instead of talking to a dead one.
    if (worker === w) worker = undefined;
    log("engine", "worker error", { message: (e.message || "").slice(0, 200) });
    const waiting = [...pending.values()];
    pending.clear();
    loadFailure(new Error(e.message || "the background engine could not be started")).then((err) =>
      waiting.forEach((p) => p.reject(err)),
    );
  };
  return w;
}

function call(method: string, args: unknown[]): Promise<unknown> {
  worker ??= start();
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    // Binary messages (from a network connection) are handed over, not copied.
    const buffers = args.flatMap((a) => (Array.isArray(a) ? a : [a])).filter((a) => a instanceof ArrayBuffer);
    worker!.postMessage({ id, method, args }, buffers);
  });
}

export function onEngineEvent(listener: (e: EngineEvent) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export const engine = new Proxy({} as EngineApi, {
  get:
    (_, method: string) =>
    (...args: unknown[]) =>
      call(method, args),
});
