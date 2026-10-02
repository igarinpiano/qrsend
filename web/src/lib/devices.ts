// This browser's device identity and its trusted devices.
import { Identity, parseDeviceId, ready } from "./core";
import * as db from "./db";

export interface DeviceInfo {
  name: string;
  fingerprint: string;
  id: string;
}

export interface Trusted extends DeviceInfo {
  added: number;
}

export async function loadIdentity(): Promise<Identity | undefined> {
  await ready();
  const secret = await db.get<string>("kv", "identity");
  return secret ? Identity.parse(secret) : undefined;
}

export async function createIdentity(name: string): Promise<Identity> {
  await ready();
  const id = Identity.generate(name);
  await db.put("kv", id.secret(), "identity");
  return id;
}

export async function renameIdentity(id: Identity, name: string): Promise<void> {
  id.rename(name);
  await db.put("kv", id.secret(), "identity");
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

export async function forget(fingerprint: string): Promise<void> {
  const list = (await trustedDevices()).filter((d) => d.fingerprint !== fingerprint);
  await db.put("kv", list, "devices");
}
