<script lang="ts">
  import { onMount } from "svelte";
  import { ready, symbolSize } from "../lib/core";
  import { trustedDevices, type Trusted } from "../lib/devices";
  import { engine, onEngineEvent } from "../lib/engine";
  import type { SendItem, SendStarted } from "../lib/engine-types";
  import { bytes } from "../lib/format";
  import { featureOn } from "../lib/prefs";
  import Player from "./Player.svelte";

  let { params }: { params: URLSearchParams } = $props();

  interface Item {
    path: string;
    file: File;
  }

  const DENSITIES = {
    auto: { version: 0, ec: "L", label: "Automatic" },
    low: { version: 15, ec: "M", label: "Low — small screens, older cameras" },
    normal: { version: 25, ec: "L", label: "Normal" },
    high: { version: 32, ec: "L", label: "High — large screens" },
    max: { version: 40, ec: "L", label: "Max — screen recordings" },
  } as const;
  type Density = keyof typeof DENSITIES;

  // Codes per side; 0 fills the screen with as many as fit.
  const GRIDS = [1, 2, 3, 4, 5, 6, 8, 0];

  let mode = $state<"files" | "text">("files");
  let items = $state<Item[]>([]);
  let text = $state("");
  let devices = $state<Trusted[]>([]);
  let selected = $state<string[]>([]);
  let anyone = $state(false);
  let density = $state<Density>("auto");
  let fps = $state(10);
  let grid = $state(1);
  let busy = $state(false);
  let packed = $state<{ done: number; total: number } | undefined>();
  let error = $state("");
  let info = $state<SendStarted | undefined>();
  /** A resume code typed in from the receiver's screen: only what it lists is sent. */
  let resume = $state("");
  let dragging = $state(false);
  let perCode = $state(0);

  onMount(() => {
    (async () => {
      devices = await trustedDevices();
      if (devices.length === 0) anyone = true;
      const shared = [params.get("title"), params.get("text"), params.get("url")].filter(Boolean).join("\n");
      const fromQuery = new URLSearchParams(location.search);
      const sharedQuery = [fromQuery.get("title"), fromQuery.get("text"), fromQuery.get("url")].filter(Boolean).join("\n");
      if (shared || sharedQuery) {
        mode = "text";
        text = shared || sharedQuery;
      }
      await ready();
      updateCapacity();
    })();
    return onEngineEvent((e) => {
      if (e.event === "send-progress") packed = { done: e.done, total: e.total };
    });
  });

  function updateCapacity() {
    const d = DENSITIES[density];
    perCode = d.version ? symbolSize(d.version, d.ec) : 0;
  }

  const total = $derived(mode === "text" ? new TextEncoder().encode(text).length : items.reduce((s, i) => s + i.file.size, 0));
  const canSend = $derived((mode === "text" ? text.length > 0 : items.length > 0) && (anyone || selected.length > 0) && !busy);

  function addFiles(list: FileList | null) {
    if (!list) return;
    const next = [...items];
    for (const file of Array.from(list)) {
      const path = (file as File & { webkitRelativePath?: string }).webkitRelativePath || file.name;
      if (!next.some((i) => i.path === path)) next.push({ path, file });
    }
    items = next;
  }

  async function readEntry(entry: FileSystemEntry, out: Item[]): Promise<void> {
    if (entry.isFile) {
      const file = await new Promise<File>((res, rej) => (entry as FileSystemFileEntry).file(res, rej));
      out.push({ path: entry.fullPath.replace(/^\//, ""), file });
    } else if (entry.isDirectory) {
      const reader = (entry as FileSystemDirectoryEntry).createReader();
      for (;;) {
        const batch = await new Promise<FileSystemEntry[]>((res, rej) => reader.readEntries(res, rej));
        if (batch.length === 0) break;
        for (const child of batch) await readEntry(child, out);
      }
    }
  }

  async function onDrop(e: DragEvent) {
    e.preventDefault();
    dragging = false;
    const entries = Array.from(e.dataTransfer?.items ?? [])
      .map((i) => i.webkitGetAsEntry?.())
      .filter((x): x is FileSystemEntry => !!x);
    if (entries.length === 0) return addFiles(e.dataTransfer?.files ?? null);
    const out: Item[] = [];
    for (const entry of entries) await readEntry(entry, out);
    const next = [...items];
    for (const i of out) if (!next.some((n) => n.path === i.path)) next.push(i);
    items = next;
    mode = "files";
  }

  /** Files plus the folders that contain them, in path order (a folder before its content). */
  function sendItems(): SendItem[] {
    const dirs = new Set<string>();
    for (const i of items) {
      const parts = i.path.split("/");
      for (let n = 1; n < parts.length; n++) dirs.add(parts.slice(0, n).join("/"));
    }
    return [...[...dirs].map((path): SendItem => ({ path })), ...items.map((i): SendItem => ({ path: i.path, file: i.file }))].sort(
      (a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0),
    );
  }

  async function start() {
    error = "";
    busy = true;
    packed = undefined;
    try {
      const d = DENSITIES[density];
      info = await engine.sendStart({
        items: mode === "text" ? [] : sendItems(),
        text: mode === "text" ? text : undefined,
        recipients: anyone ? [] : selected.map((fp) => devices.find((dev) => dev.fingerprint === fp)!.id),
        density: d.version ? { version: d.version, ec: d.ec } : null,
        redundancy: 0.1,
        resume: resume.trim() || undefined,
      });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if info}
  <Player {info} {fps} {grid} onclose={() => (info = undefined)} />
{/if}

<h2>Send</h2>

<div class="card">
  <div class="row tabs" role="tablist">
    <button role="tab" class:primary={mode === "files"} aria-selected={mode === "files"} onclick={() => (mode = "files")}>Files & folders</button>
    <button role="tab" class:primary={mode === "text"} aria-selected={mode === "text"} onclick={() => (mode = "text")}>Text</button>
  </div>

  {#if mode === "files"}
    <div
      class="drop"
      class:dragging
      role="region"
      aria-label="Drop files here"
      ondragover={(e) => {
        e.preventDefault();
        dragging = true;
      }}
      ondragleave={() => (dragging = false)}
      ondrop={onDrop}
    >
      <p>Drop files or folders here</p>
      <div class="row">
        <label class="button">
          Choose files
          <input type="file" multiple hidden onchange={(e) => addFiles(e.currentTarget.files)} />
        </label>
        <label class="button">
          Choose folder
          <input type="file" hidden webkitdirectory onchange={(e) => addFiles(e.currentTarget.files)} />
        </label>
      </div>
    </div>
    {#if items.length}
      <ul class="list small">
        {#each items as item (item.path)}
          <li class="row spread">
            <span class="ellipsis">{item.path}</span>
            <span class="row">
              <span class="muted">{bytes(item.file.size)}</span>
              <button class="danger" aria-label="Remove {item.path}" onclick={() => (items = items.filter((i) => i !== item))}>✕</button>
            </span>
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <label class="field">
      <span>Text to send</span>
      <textarea bind:value={text} placeholder="Paste or type anything…"></textarea>
    </label>
  {/if}
  <p class="muted small">Total: {bytes(total)}</p>
</div>

<div class="card">
  <h3>Who can read it?</h3>
  {#each devices as d (d.fingerprint)}
    <label class="check">
      <input type="checkbox" value={d.fingerprint} bind:group={selected} disabled={anyone} />
      <span>{d.name} <span class="muted small mono">{d.fingerprint}</span></span>
    </label>
  {/each}
  <label class="check">
    <input type="checkbox" bind:checked={anyone} />
    <span>Anyone who sees the codes <span class="badge warn">not encrypted</span></span>
  </label>
  {#if devices.length === 0}
    <p class="muted small">To encrypt, first pair the receiving device on the <a href="#/devices">Devices</a> page.</p>
  {/if}
</div>

<div class="card">
  <h3>Display</h3>
  <label class="field">
    <span>Density</span>
    <select bind:value={density} onchange={updateCapacity}>
      {#each Object.entries(DENSITIES) as [key, d]}
        <option value={key}>{d.label}</option>
      {/each}
    </select>
  </label>
  <label class="field">
    <span>Speed: {fps} codes per second</span>
    <input type="range" min="2" max="20" step="1" bind:value={fps} />
  </label>
  <label class="field">
    <span>Codes on screen</span>
    <select bind:value={grid}>
      {#each GRIDS as g}
        <option value={g}>{g === 0 ? "As many as fit the screen" : g === 1 ? "1" : `${g} × ${g}`}</option>
      {/each}
    </select>
  </label>
  <p class="muted small">
    {#if perCode && grid}
      {perCode} bytes per code · about {bytes(perCode * fps * grid * grid)}/s before losses
    {:else if perCode}
      {perCode} bytes per code
    {:else}
      The code size is chosen from the amount of data: small codes for small transfers.
    {/if}
    {#if grid !== 1}More codes at once need a sharp camera held close, or a screen recording.{/if}
  </p>
</div>

{#if error}<p class="error" role="alert">{error}</p>{/if}

<p class="small muted">
  {#if featureOn("twoWay")}
    Two-way transfer is on: the player will use this device’s camera to hear from the receiver.
  {/if}
  <a href="#/preview">Feature preview</a>
</p>

<details class="resume">
  <summary class="small">Continuing a transfer that stopped halfway?</summary>
  <p class="small muted">
    Sending the same data again continues it by itself (if it was not encrypted for a device). To send only what the
    receiver is missing, type in the resume code it shows (Inbox → How to continue):
  </p>
  <label class="field">
    <span>Resume code</span>
    <input
      type="text"
      bind:value={resume}
      placeholder="QSR1-…"
      autocapitalize="characters"
      autocomplete="off"
      spellcheck="false"
      class="mono"
    />
  </label>
</details>

<button class="primary wide" disabled={!canSend} onclick={start}>
  {#if busy}
    Preparing…{packed && packed.total ? ` ${Math.floor((packed.done / packed.total) * 100)}%` : ""}
  {:else}
    Start sending
  {/if}
</button>

<style>
  .tabs {
    margin-bottom: 12px;
  }
  .drop {
    border: 2px dashed var(--border);
    border-radius: var(--radius);
    padding: 20px;
    text-align: center;
    display: grid;
    justify-items: center;
    gap: 8px;
  }
  .drop.dragging {
    border-color: var(--accent);
  }
  .drop p {
    margin: 0;
  }
  .wide {
    width: 100%;
    min-height: 56px;
    font-size: 1.1rem;
  }
  input[type="range"] {
    width: 100%;
  }
</style>
