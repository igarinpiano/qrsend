// Minimal IndexedDB helpers. Small records only: the device identity (as
// non-extractable keys), trusted devices and the list of receive sessions.
// Transfer data itself lives in OPFS files (see storage.ts).

const DB_NAME = "qrsend";
const VERSION = 2;

let opening: Promise<IDBDatabase> | undefined;

function db(): Promise<IDBDatabase> {
  opening ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, VERSION);
    req.onupgradeneeded = (e) => {
      const d = req.result;
      // Whatever is there (nothing, or a v1 database): end up with these stores.
      const had = (name: string) => d.objectStoreNames.contains(name);
      const hadSessions = had("sessions");
      if (!had("kv")) d.createObjectStore("kv");
      if (!hadSessions) d.createObjectStore("sessions", { keyPath: "session" });
      if (e.oldVersion === 1) {
        // v1 kept received segments in IndexedDB; they are in OPFS now, and
        // unfinished v1 sessions cannot be continued.
        if (had("segments")) d.deleteObjectStore("segments");
        if (hadSessions) req.transaction!.objectStore("sessions").clear();
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
  return opening;
}

function wrap<T>(req: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

async function store(name: string, mode: IDBTransactionMode = "readonly"): Promise<IDBObjectStore> {
  return (await db()).transaction(name, mode).objectStore(name);
}

export async function get<T>(name: string, key: IDBValidKey): Promise<T | undefined> {
  return wrap((await store(name)).get(key));
}

export async function put(name: string, value: unknown, key?: IDBValidKey): Promise<void> {
  await wrap((await store(name, "readwrite")).put(value, key));
}

export async function del(name: string, key: IDBValidKey | IDBKeyRange): Promise<void> {
  await wrap((await store(name, "readwrite")).delete(key));
}

export async function all<T>(name: string, range?: IDBKeyRange): Promise<T[]> {
  return wrap((await store(name)).getAll(range));
}

export async function keys(name: string, range?: IDBKeyRange): Promise<IDBValidKey[]> {
  return wrap((await store(name)).getAllKeys(range));
}

/** Asks the browser not to evict our data under storage pressure. */
export async function persist(): Promise<boolean> {
  try {
    return (await navigator.storage?.persist?.()) ?? false;
  } catch {
    return false;
  }
}

export async function estimate(): Promise<{ usage: number; quota: number } | undefined> {
  try {
    const e = await navigator.storage?.estimate?.();
    return e ? { usage: e.usage ?? 0, quota: e.quota ?? 0 } : undefined;
  } catch {
    return undefined;
  }
}
