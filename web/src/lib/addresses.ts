// What an address says about the network a connection runs over.
//
// A direct connection is made between whatever addresses the two devices
// have. On one Wi-Fi those are addresses of that network. But a phone on
// mobile data has an address of the internet at large (IPv6), and two such
// phones connect just the same, through the internet and on their data
// plans. No browser on an iPhone says which kind of network it is on; the
// addresses do say whether two devices are on the same one.
//
// (Pure logic, no browser needed: e2e/addresses.spec.ts tests it in Node.)

/** The kind of an address (the address itself is nobody's business: this is what the log records). */
export function addressKind(address: string | undefined | null): string {
  if (!address) return "hidden";
  if (address.endsWith(".local")) return "name";
  if (address.includes(":")) {
    const a = address.toLowerCase();
    if (a === "::1") return "v6 loopback";
    if (/^fe[89ab]/.test(a)) return "v6 link-local";
    if (/^f[cd]/.test(a)) return "v6 private";
    if (/^[23]/.test(a)) return "v6 global";
    return "v6 other";
  }
  const [x, y] = address.split(".").map(Number);
  if (x === 127) return "v4 loopback";
  if (x === 10 || (x === 172 && y >= 16 && y <= 31) || (x === 192 && y === 168)) return "v4 private";
  if (x === 169 && y === 254) return "v4 link-local";
  if (x === 100 && y >= 64 && y <= 127) return "v4 carrier";
  if (x === 192 && y === 0) return "v4 transition";
  return "v4 public";
}

/** Kinds of addresses that exist within one network only. */
const WITHIN = new Set(["name", "v4 private", "v4 link-local", "v4 loopback", "v6 link-local", "v6 private", "v6 loopback"]);

/**
 * The network part of an IPv6 address: its first 64 bits. Devices on one Wi-Fi share it; every phone on mobile data
 * has one of its own.
 */
export function network64(address: string): string | null {
  if (!address.includes(":") || address.includes(".")) return null;
  const [head, tail] = address.toLowerCase().split("%")[0].split("::");
  const left = head ? head.split(":") : [];
  const right = tail !== undefined && tail !== "" ? tail.split(":") : [];
  if (tail === undefined ? left.length !== 8 : left.length + right.length > 7) return null;
  const all = [...left, ...new Array(8 - left.length - right.length).fill("0"), ...right];
  if (!all.every((g) => /^[0-9a-f]{1,4}$/.test(g))) return null;
  return all
    .slice(0, 4)
    .map((g) => parseInt(g, 16).toString(16))
    .join(":");
}

/**
 * Those of `theirs` that can be on one network with a device whose addresses are `ours`: addresses that only exist
 * within a network, and addresses of the internet at large that share their network part with one of ours. (A name
 * counts: only a device on the same network can find out what it stands for.)
 */
export function onOneNetwork<T extends { address: string }>(theirs: T[], ours: { address: string }[]): T[] {
  const networks = new Set(ours.map((c) => network64(c.address)).filter(Boolean));
  return theirs.filter((c) => {
    const kind = addressKind(c.address);
    if (WITHIN.has(kind)) return true;
    return kind === "v6 global" && networks.has(network64(c.address));
  });
}

export type Path = "local" | "internet" | "unknown";

/** Over what a connection between these two addresses runs. */
export function pathBetween(own: string | undefined | null, other: string | undefined | null): Path {
  const [a, b] = [addressKind(own), addressKind(other)];
  // One end within a network: so is the other (or there would be no connection).
  if ((WITHIN.has(a) && a !== "name") || (WITHIN.has(b) && b !== "name")) return "local";
  if (a === "v6 global" && b === "v6 global") return network64(own!) === network64(other!) ? "local" : "internet";
  if ([a, b].some((k) => k === "v4 public" || k === "v4 carrier" || k === "v4 transition")) return "internet";
  return "unknown";
}
