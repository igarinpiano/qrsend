<script lang="ts">
  import { onMount } from "svelte";
  import { SendBuilder, ready, symbolSize, type SendSession } from "../lib/core";
  import { loadIdentity, trustedDevices, type Trusted } from "../lib/devices";
  import { bytes } from "../lib/format";
  import Player from "./Player.svelte";

  let { params }: { params: URLSearchParams } = $props();

  interface Item {
    path: string;
    file: File;
  }

  const DENSITIES = {
    low: { version: 15, ec: "M", label: "Low — small screens, older cameras" },
    normal: { version: 25, ec: "L", label: "Normal" },
    high: { version: 32, ec: "L", label: "High — large screens" },
    max: { version: 40, ec: "L", label: "Max — screen recordings" },
  } as const;
  type Density = keyof typeof DENSITIES;

  let mode = $state<"files" | "text">("files");
  let items = $state<Item[]>([]);
  let text = $state("");
  let devices = $state<Trusted[]>([]);
  let selected = $state<string[]>([]);
  let anyone = $state(false);
  let density = $state<Density>("normal");
  let fps = $state(10);
  let grid = $state(1);
  let busy = $state(false);
  let error = $state("");
  let session = $state<SendSession | undefined>();
  let dragging = $state(false);
  let perCode = $state(0);

  onMount(async () => {
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
  });

  function updateCapacity() {
    const d = DENSITIES[density];
    perCode = symbolSize(d.version, d.ec);
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

  async function start() {
    error = "";
    busy = true;
    try {
      await ready();
      const builder = new SendBuilder();
      if (mode === "text") {
        builder.setText(text);
      } else {
        const dirs = new Set<string>();
        for (const i of items) {
          const parts = i.path.split("/");
          for (let n = 1; n < parts.length; n++) dirs.add(parts.slice(0, n).join("/"));
        }
        const entries = [
          ...[...dirs].map((path) => ({ path, file: undefined as File | undefined })),
          ...items.map((i) => ({ path: i.path, file: i.file as File | undefined })),
        ].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
        for (const e of entries) {
          if (!e.file) builder.addDir(e.path);
          else builder.addFile(e.path, new Uint8Array(await e.file.arrayBuffer()), e.file.lastModified);
        }
      }
      if (!anyone) for (const fp of selected) builder.addRecipient(devices.find((d) => d.fingerprint === fp)!.id);
      const me = await loadIdentity();
      const d = DENSITIES[density];
      session = builder.build(me, d.version, d.ec, 0.1);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if session}
  <Player {session} {fps} {grid} onclose={() => (session = undefined)} />
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
  <p class="muted small">Total: {bytes(total)}{total > 400 * 1024 * 1024 ? " — large transfers are better sent with the qrsend CLI" : ""}</p>
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
      <option value={1}>1</option>
      <option value={2}>2 × 2 (large screens)</option>
    </select>
  </label>
  <p class="muted small">{perCode} bytes per code · about {bytes(perCode * fps * grid * grid)}/s before losses</p>
</div>

{#if error}<p class="error">{error}</p>{/if}

<button class="primary wide" disabled={!canSend} onclick={start}>{busy ? "Preparing…" : "Start sending"}</button>

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
