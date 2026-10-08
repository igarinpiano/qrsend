// This device's lasting certificate for direct connections (a preview
// feature: connecting to known devices without an answer code).
//
// A connection's certificate is normally made for that connection and then
// forgotten. A sender that remembers the devices it trusts needs to know
// the receiver's certificate before connecting, so a receiver that meets
// such a sender makes one certificate and keeps it. Its private key never
// leaves the browser (it cannot even be read by this page).
import * as db from "./db";

const KEY = "link-certificate";
const VALID_MS = 365 * 86_400_000;
/** A certificate this close to its end is replaced. */
const RENEW_MS = 30 * 86_400_000;

export interface Lasting {
  certificate: RTCCertificate;
  /** SHA-256 fingerprint. */
  fingerprint: Uint8Array;
}

function fingerprintOf(certificate: RTCCertificate): Uint8Array | undefined {
  const value = certificate.getFingerprints?.().find((f) => f.algorithm?.toLowerCase() === "sha-256")?.value;
  if (!value) return undefined;
  const bytes = value.split(":").map((b) => parseInt(b, 16));
  return bytes.length === 32 && bytes.every((b) => b >= 0 && b <= 255) ? new Uint8Array(bytes) : undefined;
}

let made: Promise<Lasting | undefined> | undefined;

/**
 * The certificate kept by this device, or undefined when it has none (`create` false) or cannot keep one. Never
 * throws: without a lasting certificate, connections are made the usual way.
 */
export async function lastingCertificate(create: boolean): Promise<Lasting | undefined> {
  try {
    const stored = await db.get<{ certificate?: RTCCertificate }>("kv", KEY);
    const kept = stored?.certificate;
    if (kept && kept.expires !== null && kept.expires - Date.now() > RENEW_MS) {
      const fingerprint = fingerprintOf(kept);
      if (fingerprint) return { certificate: kept, fingerprint };
    }
    if (!create) return undefined;
    made ??= (async () => {
      const certificate = await RTCPeerConnection.generateCertificate({
        name: "ECDSA",
        namedCurve: "P-256",
        expires: VALID_MS,
      } as AlgorithmIdentifier);
      const fingerprint = fingerprintOf(certificate);
      if (!fingerprint) return undefined;
      await db.put("kv", { certificate }, KEY);
      // Some browsers accept what they cannot keep: only a certificate that reads back counts.
      const back = await db.get<{ certificate?: RTCCertificate }>("kv", KEY);
      return back?.certificate && fingerprintOf(back.certificate) ? { certificate, fingerprint } : undefined;
    })();
    return await made;
  } catch {
    return undefined;
  }
}
