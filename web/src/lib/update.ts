// Keeping a page and the deployed app in step.
//
// Every build names its files after their content. A page that was opened
// (or cached by the service worker) before a new version went live may later
// ask for a file of its own build — the engine's WebAssembly, a worker —
// that the server no longer has, and gets a 404. The cure is to load the new
// version; this module finds out when that is the case and does it.

const RELOADED_AT = "qrsend.reloadedAt";
/** Do not reload again within this time: a reload that did not help must not loop. */
const RELOAD_GUARD_MS = 60_000;

/** The entry script this page runs. */
function ownScript(): string | undefined {
  return [...document.scripts].find((s) => s.type === "module" && s.src)?.src;
}

/** Whether the server now has another build than the one this page runs. */
export async function isStale(): Promise<boolean> {
  try {
    // The query keeps the service worker's cache out of it: this must be the
    // server's answer.
    const response = await fetch(`./index.html?fresh=${Date.now()}`, { cache: "no-store" });
    if (!response.ok) return false;
    const match = (await response.text()).match(/<script[^>]+type="module"[^>]+src="([^"]+)"/);
    const mine = ownScript();
    return !!match && !!mine && new URL(match[1], location.href).href !== mine;
  } catch {
    return false;
  }
}

/** Loads the current version, unless that was tried a moment ago. Returns false if it was. */
export async function reloadForUpdate(): Promise<boolean> {
  try {
    const last = Number(sessionStorage.getItem(RELOADED_AT) ?? 0);
    if (Date.now() - last < RELOAD_GUARD_MS) return false;
    sessionStorage.setItem(RELOADED_AT, String(Date.now()));
  } catch {
    /* no storage: reload anyway, once per call */
  }
  try {
    await (await navigator.serviceWorker?.getRegistration())?.update();
  } catch {
    /* offline or unsupported: the reload itself still helps */
  }
  location.reload();
  return true;
}

/** What to tell the person when a part of the app could not be loaded and reloading did not happen. */
export const RELOAD_HINT = "Reload the page to try again.";

/**
 * A part of the app failed to load. If that is because a new version went
 * live, the page reloads into it (this never returns then). Otherwise the
 * caller should try again or report the failure.
 */
export async function recoverFromLoadFailure(): Promise<void> {
  if (await isStale()) {
    if (await reloadForUpdate()) await new Promise(() => {});
  }
}
