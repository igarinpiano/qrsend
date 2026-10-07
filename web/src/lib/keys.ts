// This browser's device identity.
//
// Private keys are WebCrypto keys created as non-extractable: scripts (ours
// included) can use them to decrypt and sign but can never read them out.
// Browsers that cannot do that fall back to keys held by the WebAssembly core
// ("legacy"): those without WebCrypto X25519 / Ed25519, and those that have
// them but cannot keep such keys in IndexedDB (WebKit, i.e. Safari and every
// browser on iOS, accepts them and then returns nothing). Identities stored
// by earlier versions are imported into WebCrypto on first use, keeping the
// same device ID.
import { Identity, deviceInfo, ready } from "./core";
import * as db from "./db";

export interface DeviceInfo {
  name: string;
  fingerprint: string;
  id: string;
}

export type StoredIdentity =
  | {
      kind: "webcrypto";
      name: string;
      x25519Public: Uint8Array;
      ed25519Public: Uint8Array;
      x25519Private: CryptoKey;
      ed25519Private: CryptoKey;
    }
  | { kind: "legacy"; secret: string };

const KEY = "identity";
const PROBE_KEY = "identity-probe";

let storable: Promise<boolean> | undefined;

/**
 * Whether this browser can create these keys, keep them in IndexedDB and use
 * them after reading them back. Tried with a throwaway key, never with the
 * real identity: a browser that silently drops stored keys would lose it.
 */
export function canKeepKeys(): Promise<boolean> {
  storable ??= (async () => {
    try {
      const pair = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
      const x = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
      await db.put("kv", { sign: pair.privateKey, derive: x.privateKey }, PROBE_KEY);
      const back = await db.get<{ sign?: CryptoKey; derive?: CryptoKey }>("kv", PROBE_KEY);
      if (!back?.sign || !back.derive) return false;
      await crypto.subtle.sign({ name: "Ed25519" }, back.sign, new Uint8Array(1));
      await crypto.subtle.deriveBits({ name: "X25519", public: x.publicKey }, back.derive, 256);
      return true;
    } catch {
      return false;
    } finally {
      await db.del("kv", PROBE_KEY).catch(() => {});
    }
  })();
  return storable;
}

function b64url(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

const raw = (buf: ArrayBuffer | Uint8Array) => new Uint8Array(buf instanceof Uint8Array ? buf : buf);
const bytes = (u: Uint8Array) => u as Uint8Array<ArrayBuffer>;

async function generateWebCrypto(name: string): Promise<StoredIdentity> {
  const x = (await crypto.subtle.generateKey({ name: "X25519" }, false, ["deriveBits"])) as CryptoKeyPair;
  const ed = (await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"])) as CryptoKeyPair;
  return {
    kind: "webcrypto",
    name,
    x25519Public: raw(await crypto.subtle.exportKey("raw", x.publicKey)),
    ed25519Public: raw(await crypto.subtle.exportKey("raw", ed.publicKey)),
    x25519Private: x.privateKey,
    ed25519Private: ed.privateKey,
  };
}

/** Imports keys held by the WebAssembly core into non-extractable WebCrypto keys. */
async function importLegacy(secret: string): Promise<StoredIdentity> {
  const legacy = Identity.parse(secret);
  const info = legacy.info() as DeviceInfo;
  const k = legacy.rawKeys() as {
    x25519Secret: Uint8Array;
    x25519Public: Uint8Array;
    ed25519Seed: Uint8Array;
    ed25519Public: Uint8Array;
  };
  const jwk = (crv: string, d: Uint8Array, x: Uint8Array): JsonWebKey => ({ kty: "OKP", crv, d: b64url(d), x: b64url(x) });
  const x25519Private = await crypto.subtle.importKey(
    "jwk",
    jwk("X25519", k.x25519Secret, k.x25519Public),
    { name: "X25519" },
    false,
    ["deriveBits"],
  );
  const ed25519Private = await crypto.subtle.importKey(
    "jwk",
    jwk("Ed25519", k.ed25519Seed, k.ed25519Public),
    { name: "Ed25519" },
    false,
    ["sign"],
  );
  return {
    kind: "webcrypto",
    name: info.name,
    x25519Public: k.x25519Public,
    ed25519Public: k.ed25519Public,
    x25519Private,
    ed25519Private,
  };
}

export async function loadIdentity(): Promise<StoredIdentity | undefined> {
  await ready();
  const stored = await db.get<StoredIdentity | string>("kv", KEY);
  if (!stored) return undefined;
  // Earlier versions stored the secret as a string.
  const legacySecret = typeof stored === "string" ? stored : stored.kind === "legacy" ? stored.secret : undefined;
  if (legacySecret === undefined) return stored as StoredIdentity;
  const legacy: StoredIdentity = { kind: "legacy", secret: legacySecret };
  if (!(await canKeepKeys())) return legacy;
  try {
    const migrated = await importLegacy(legacySecret);
    await db.put("kv", migrated, KEY);
    return migrated;
  } catch {
    return legacy;
  }
}

export async function createIdentity(name: string): Promise<StoredIdentity> {
  await ready();
  let id: StoredIdentity | undefined;
  if (await canKeepKeys()) {
    try {
      id = await generateWebCrypto(name);
    } catch {
      /* fall back below */
    }
  }
  id ??= { kind: "legacy", secret: Identity.generate(name).secret() };
  await db.put("kv", id, KEY);
  return id;
}

export async function renameIdentity(id: StoredIdentity, name: string): Promise<StoredIdentity> {
  let next: StoredIdentity;
  if (id.kind === "webcrypto") {
    next = { ...id, name };
  } else {
    const legacy = Identity.parse(id.secret);
    legacy.rename(name);
    next = { kind: "legacy", secret: legacy.secret() };
  }
  await db.put("kv", next, KEY);
  return next;
}

export function identityInfo(id: StoredIdentity): DeviceInfo {
  return (
    id.kind === "webcrypto"
      ? deviceInfo(id.name, id.x25519Public, id.ed25519Public)
      : Identity.parse(id.secret).info()
  ) as DeviceInfo;
}

/** X25519(our private key, their ephemeral public key). WebCrypto identities only. */
export async function sharedSecret(id: StoredIdentity & { kind: "webcrypto" }, ephemeral: Uint8Array): Promise<Uint8Array> {
  const theirs = await crypto.subtle.importKey("raw", bytes(ephemeral), { name: "X25519" }, false, []);
  return raw(await crypto.subtle.deriveBits({ name: "X25519", public: theirs }, id.x25519Private, 256));
}

/** Ed25519 signature and the signer's public key. */
export async function sign(id: StoredIdentity, message: Uint8Array): Promise<{ signature: Uint8Array; signer: Uint8Array }> {
  if (id.kind === "webcrypto") {
    const signature = raw(await crypto.subtle.sign({ name: "Ed25519" }, id.ed25519Private, bytes(message)));
    return { signature, signer: id.ed25519Public };
  }
  const legacy = Identity.parse(id.secret);
  const keys = legacy.rawKeys() as { ed25519Public: Uint8Array };
  return { signature: legacy.sign(message), signer: keys.ed25519Public };
}
