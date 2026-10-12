// The devices this browser trusts (their public IDs only).
import { parseDeviceId, ready } from "./core";
import * as db from "./db";
import type { DeviceInfo } from "./keys";

export type { DeviceInfo };

export interface Trusted extends DeviceInfo {
  added: number;
  /**
   * What is needed to connect to this device over the local network without its answer code: the fingerprint (hex)
   * of the certificate it connected with last time, and when that was. See `lan.ts`.
   */
  link?: { certificate: string; seen: number };
}

export async function trustedDevices(): Promise<Trusted[]> {
  const list = (await db.get<Trusted[]>("kv", "devices")) ?? [];
  // (Records from before post-quantum keys do not say what kind they are.)
  if (list.some((d) => d.classic === undefined)) await ready();
  return list.map((d) => (d.classic === undefined ? { ...d, ...(parseDeviceId(d.id) as DeviceInfo), name: d.name } : d));
}

export async function inspect(idString: string): Promise<DeviceInfo> {
  await ready();
  return parseDeviceId(idString.trim()) as DeviceInfo;
}

/**
 * Trusts a device. One that is trusted with its older keys only and now shows its ID with post-quantum keys is not
 * added a second time: its record takes the new keys (and keeps what else is known of it). Returns "upgraded" then.
 */
export async function trust(info: DeviceInfo, name: string): Promise<"added" | "upgraded"> {
  const list = await trustedDevices();
  const known = list.find((d) => d.classic === info.classic);
  if (known) {
    if (known.fingerprint === info.fingerprint) throw new Error("This device is already trusted.");
    if (known.pq && !info.pq) {
      throw new Error(`This device is already trusted as “${known.name}”, with post-quantum keys; this is its older ID.`);
    }
    if (known.pq && info.pq) {
      throw new Error(
        `“${known.name}” is trusted with other post-quantum keys than this ID shows. If the device made itself new keys, remove it first; if not, this is not its ID.`,
      );
    }
    if (list.some((d) => d !== known && d.name === name)) throw new Error(`A device is already named “${name}”.`);
    Object.assign(known, info, { name });
    await db.put("kv", list, "devices");
    return "upgraded";
  }
  if (list.some((d) => d.name === name)) throw new Error(`A device is already named “${name}”.`);
  list.push({ ...info, name, added: Date.now() });
  await db.put("kv", list, "devices");
  return "added";
}

/** Remembers the certificate a trusted device connected with. Returns its name, or undefined if it is not trusted. */
export async function rememberLink(fingerprint: string, certificate: string): Promise<string | undefined> {
  const list = await trustedDevices();
  const device = list.find((d) => d.fingerprint === fingerprint);
  if (!device) return undefined;
  device.link = { certificate, seen: Date.now() };
  await db.put("kv", list, "devices");
  return device.name;
}

export async function forget(fingerprint: string): Promise<void> {
  const list = (await trustedDevices()).filter((d) => d.fingerprint !== fingerprint);
  await db.put("kv", list, "devices");
}
