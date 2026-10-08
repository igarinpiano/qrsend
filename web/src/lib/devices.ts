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
  return (await db.get<Trusted[]>("kv", "devices")) ?? [];
}

export async function inspect(idString: string): Promise<DeviceInfo> {
  await ready();
  return parseDeviceId(idString.trim()) as DeviceInfo;
}

export async function trust(info: DeviceInfo, name: string): Promise<void> {
  const list = await trustedDevices();
  if (list.some((d) => d.fingerprint === info.fingerprint)) throw new Error("This device is already trusted.");
  if (list.some((d) => d.name === name)) throw new Error(`A device is already named “${name}”.`);
  list.push({ ...info, name, added: Date.now() });
  await db.put("kv", list, "devices");
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
