// Page-side handle to the engine worker (packing, frame generation, receive
// state, storage and extraction all run there, off the UI thread).
import EngineWorker from "./engine.worker?worker";
import type { EngineApi, EngineEvent } from "./engine-types";

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

let worker: Worker | undefined;
let nextId = 1;
const pending = new Map<number, Pending>();
const listeners = new Set<(e: EngineEvent) => void>();

function start(): Worker {
  const w = new EngineWorker();
  w.onmessage = (e: MessageEvent) => {
    const msg = e.data;
    if (msg.event) {
      listeners.forEach((l) => l(msg as EngineEvent));
      return;
    }
    const p = pending.get(msg.id);
    if (!p) return;
    pending.delete(msg.id);
    if (msg.error !== undefined) p.reject(new Error(msg.error));
    else p.resolve(msg.result);
  };
  w.onerror = (e) => {
    const err = new Error(e.message || "The background engine stopped unexpectedly.");
    pending.forEach((p) => p.reject(err));
    pending.clear();
  };
  return w;
}

function call(method: string, args: unknown[]): Promise<unknown> {
  worker ??= start();
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    worker!.postMessage({ id, method, args });
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
