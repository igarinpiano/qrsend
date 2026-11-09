// Where another device opens QRSend to receive: the address of this web app,
// with its Receive page. Shown as an ordinary QR code that any phone's
// camera app opens, for a receiver who has no QRSend open yet.

/** The published web app. */
export const PUBLISHED = "https://igarinpiano.github.io/qrsend/";

/** Hosts that only this device (or its own network) can reach. */
function onlyHere(host: string): boolean {
  const h = host.replace(/^\[|\]$/g, "").toLowerCase();
  return (
    h === "localhost" ||
    h.endsWith(".localhost") ||
    h.endsWith(".local") ||
    h === "::1" ||
    /^127\./.test(h) ||
    /^10\./.test(h) ||
    /^192\.168\./.test(h) ||
    /^172\.(1[6-9]|2\d|3[01])\./.test(h) ||
    /^169\.254\./.test(h) ||
    /^f[cd][0-9a-f]{2}:/.test(h) ||
    /^fe80:/.test(h)
  );
}

/**
 * The address that opens the Receive page on another device. It is this
 * copy of the app where others can reach it (the published site, or a copy
 * someone hosts), and the published site where they cannot: a file opened
 * from the offline download, a development server, an address on the local
 * network (served without https, where a browser gives no camera anyway).
 */
export function receiveLink(at: { protocol: string; hostname: string; origin: string; pathname: string } = location): string {
  const reachable = at.protocol === "https:" && !onlyHere(at.hostname);
  const base = reachable ? at.origin + at.pathname.replace(/[^/]*$/, "") : PUBLISHED;
  return `${base}#/receive`;
}
