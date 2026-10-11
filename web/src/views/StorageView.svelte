<script lang="ts">
  import { onMount } from "svelte";
  import { estimate } from "../lib/db";
  import { engine } from "../lib/engine";
  import type { StorageItem } from "../lib/engine-types";
  import { bytes } from "../lib/format";
  import { clearLog, log, logBytes, logLines } from "../lib/log";

  // What the page lists: the things in the file storage (from the engine),
  // and the diagnostic log, which lives in the page.
  type Thing = { id: string; title: string; detail: string; bytes: number; fixed?: boolean };
  const LOG = "log";

  let things = $state<Thing[]>([]);
  let picked = $state<Record<string, boolean>>({});
  let storage = $state<{ usage: number; quota: number } | undefined>();
  let loaded = $state(false);
  let busy = $state(false);
  let message = $state("");

  function describe(item: StorageItem): Thing {
    const rec = item.record;
    const when = rec ? new Date(rec.updated).toLocaleString() : "";
    switch (item.kind) {
      case "received":
        return { id: item.id, bytes: item.bytes, title: rec?.summary ?? "Unknown transfer", detail: `Received, complete · ${when}` };
      case "unfinished":
        return {
          id: item.id,
          bytes: item.bytes,
          title: rec?.summary ?? "Unknown transfer",
          detail: `Received in part (${rec?.doneCount} of ${rec?.total}) · ${when}`,
        };
      case "sending":
        return { id: item.id, bytes: item.bytes, title: "The transfer being sent now", detail: "Goes by itself when sending ends", fixed: true };
      default:
        return { id: item.id, bytes: item.bytes, title: "Left over", detail: "Of a transfer that is gone, or of a tab closed while it was sending" };
    }
  }

  async function load() {
    const items = (await engine.storageList()).map(describe).sort((a, b) => b.bytes - a.bytes);
    const lines = logLines();
    things = [...items, { id: LOG, bytes: logBytes(), title: "Diagnostic log", detail: `What “Copy log” copies · ${lines} line${lines === 1 ? "" : "s"}` }];
    picked = Object.fromEntries(things.filter((t) => picked[t.id] && !t.fixed).map((t) => [t.id, true]));
    storage = await estimate();
    loaded = true;
  }
  onMount(() => {
    load().catch((e) => (message = e instanceof Error ? e.message : String(e)));
  });

  const removable = $derived(things.filter((t) => !t.fixed));
  const chosen = $derived(removable.filter((t) => picked[t.id]));
  const sum = (list: Thing[]) => list.reduce((n, t) => n + t.bytes, 0);
  const allPicked = $derived(removable.length > 0 && chosen.length === removable.length);

  function pickAll() {
    picked = allPicked ? {} : Object.fromEntries(removable.map((t) => [t.id, true]));
  }

  async function remove() {
    const list = chosen;
    const freed = sum(list);
    const what = list.length === 1 ? `“${list[0].title}”` : `${list.length} things`;
    if (!confirm(`Delete ${what} (${bytes(freed)})? This cannot be undone.`)) return;
    busy = true;
    message = "";
    try {
      await engine.storageRemove(list.filter((t) => t.id !== LOG).map((t) => t.id));
      if (list.some((t) => t.id === LOG)) clearLog();
      log("store", "removed on the storage page", { things: list.length, bytes: freed });
      picked = {};
      await load();
      message = `Removed ${bytes(freed)}.`;
    } catch (e) {
      message = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<h2>Storage</h2>
<p class="muted">What QRSend keeps in this browser, and the room each thing takes. Pick what to delete, or all of it.</p>

{#if loaded}
  <p data-testid="storage-total">QRSend keeps <strong>{bytes(sum(things))}</strong> in this browser.</p>
  <ul class="list card">
    {#each things as thing (thing.id)}
      <li>
        <label class="thing">
          <input type="checkbox" bind:checked={picked[thing.id]} disabled={thing.fixed || busy} />
          <span class="what">
            <strong class="ellipsis">{thing.title}</strong>
            <span class="small muted">{thing.detail}</span>
          </span>
          <span class="size">{bytes(thing.bytes)}</span>
        </label>
      </li>
    {/each}
  </ul>
  <div class="row">
    <button onclick={pickAll} disabled={busy || removable.length === 0}>{allPicked ? "Select none" : "Select all"}</button>
    <button class="danger" onclick={remove} disabled={busy || chosen.length === 0}>
      {chosen.length === 0 ? "Delete selected" : `Delete selected (${chosen.length} · ${bytes(sum(chosen))})`}
    </button>
  </div>
{/if}

{#if message}<p class="muted" role="status">{message}</p>{/if}

{#if storage}
  <p class="small muted">
    Browser storage used: {bytes(storage.usage)} of {bytes(storage.quota)}, as the browser reckons it. That includes the app
    itself, kept so that it works offline.
  </p>
{/if}
<p class="small muted">
  A transfer you received stays in the <a href="#/inbox">inbox</a> after you save its files, until you delete it. What you send
  is kept only while it is being sent.
</p>
<p class="small muted">
  Never deleted here: this device's ID, the trusted devices and your settings. Clearing this site's data in the browser's
  settings removes those too — the device ID does not come back, and other devices would have to trust the new one again.
</p>

<style>
  /* (A list's card has no padding of its own.) */
  .thing {
    display: flex;
    gap: 12px;
    align-items: center;
    padding: 0 14px;
    cursor: pointer;
  }
  .thing input {
    flex: none;
    width: 20px;
    height: 20px;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .size {
    flex: none;
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }
  .row {
    margin: 12px 0 16px;
  }
</style>
