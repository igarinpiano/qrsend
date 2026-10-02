// Receive sessions persisted in IndexedDB so transfers survive reloads and
// can be resumed later.
import { Receive, ready } from "./core";
import * as db from "./db";
import { trustedDevices, loadIdentity } from "./devices";

export interface SessionRecord {
  session: string;
  flags: number;
  segShift: number;
  segCount: number;
  created: number;
  updated: number;
  done: number;
  total: number;
  complete: boolean;
  summary?: string;
}

const segKey = (session: string, index: number) => `${session}:${String(index).padStart(8, "0")}`;
const range = (session: string) => IDBKeyRange.bound(`${session}:`, `${session}:￿`);

export function listSessions(): Promise<SessionRecord[]> {
  return db.all<SessionRecord>("sessions").then((l) => l.sort((a, b) => b.updated - a.updated));
}

export function getSession(session: string): Promise<SessionRecord | undefined> {
  return db.get<SessionRecord>("sessions", session);
}

export async function saveRecord(r: SessionRecord): Promise<void> {
  await db.put("sessions", { ...r, updated: Date.now() });
}

export async function saveSegment(session: string, index: number, data: Uint8Array): Promise<void> {
  await db.put("segments", data, segKey(session, index));
}

export async function deleteSession(session: string): Promise<void> {
  await db.del("segments", range(session));
  await db.del("sessions", session);
}

/** A Receive with identity and trusted devices loaded. */
export async function newReceive(session?: string): Promise<Receive> {
  await ready();
  const r = new Receive(session);
  const me = await loadIdentity();
  if (me) r.setIdentity(me);
  for (const d of await trustedDevices()) r.addTrusted(d.id, d.name);
  return r;
}

/** Rebuilds a Receive from what was stored for `session`. */
export async function restore(session: string): Promise<Receive> {
  const rec = await getSession(session);
  if (!rec) throw new Error("Unknown session");
  const r = await newReceive(session);
  r.restore(rec.session, rec.flags, rec.segShift, rec.segCount);
  const keys = (await db.keys("segments", range(session))) as string[];
  // Meta first so body segments can be verified as they are restored.
  keys.sort((a, b) => a.localeCompare(b));
  for (const key of keys) {
    const index = Number(key.split(":")[1]);
    const data = await db.get<Uint8Array>("segments", key);
    if (data) r.restoreSegment(index, data);
  }
  return r;
}
