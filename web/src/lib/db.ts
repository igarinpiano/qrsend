// Minimal IndexedDB helpers. Everything QRSend keeps lives here, in this
// browser only: the device identity, trusted devices and received segments.

const DB_NAME = "qrsend";
const VERSION = 1;

let opening: Promise<IDBDatabase> | undefined;

function db(): Promise<IDBDatabase> {
  opening ??= new Promise((resolve, reject) => {
    const req = indexedDB.open(DB_NAME, VERSION);
    req.onupgradeneeded = () => {
      const d = req.result;
      d.createObjectStore("kv");
      d.createObjectStore("sessions", { keyPath: "session" });
      d.createObjectStore("segments");
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
