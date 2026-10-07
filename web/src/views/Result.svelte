<script lang="ts">
  import { onMount } from "svelte";
  import { engine } from "../lib/engine";
  import type { RecvResult } from "../lib/engine-types";
  import { bytes } from "../lib/format";
  import { basename, canPickFolder, copyText, download, saveToFolder, sliceEntries, zip, type FileEntry } from "../lib/save";

  let { result }: { result: RecvResult } = $props();

  let message = $state("");
  let entries = $state<FileEntry[]>([]);
  let busy = $state(false);
  const name = $derived(`qrsend-${result.session}`);
  const files = $derived(entries.filter((e) => !e.dir));

  onMount(async () => {
    if (result.kind !== "files") return;
    try {
      // Entries are byte ranges of one file on disk; slicing reads nothing.
      entries = sliceEntries(await engine.outBlob(result.session), result.entries);
    } catch (e) {
      message = e instanceof Error ? e.message : String(e);
    }
  });

  async function copy(text: string) {
    await copyText(text);
    message = "Copied to the clipboard.";
  }

  async function toFolder() {
    busy = true;
    try {
      const n = await saveToFolder(entries);
      message = `Saved ${n} file${n === 1 ? "" : "s"}.`;
    } catch (e) {
      if (!(e instanceof DOMException && e.name === "AbortError")) message = String(e);
    } finally {
      busy = false;
    }
  }

  async function asZip() {
    busy = true;
    message = "Preparing the ZIP…";
    try {
      download(await zip(entries), `${name}.zip`);
      message = "";
    } catch (e) {
      message = String(e);
    } finally {
      busy = false;
    }
  }
</script>

{#if result.kind === "text"}
  <div class="card stack">
    <h3>Received text</h3>
    <pre data-testid="received-text">{result.text}</pre>
    <div class="row">
      <button class="primary" onclick={() => copy(result.text)}>Copy</button>
      <button onclick={() => download(new Blob([result.text], { type: "text/plain" }), `${name}.txt`)}>Save as file</button>
    </div>
  </div>
{:else}
  <div class="card stack">
    <h3>Received {files.length} file{files.length === 1 ? "" : "s"}</h3>
    <div class="row">
      {#if files.length === 1 && !files[0].path.includes("/")}
        <button class="primary" onclick={() => download(files[0].blob!, basename(files[0].path))}>Save {basename(files[0].path)}</button>
      {:else if files.length}
        {#if canPickFolder}<button class="primary" disabled={busy} onclick={toFolder}>Save to folder…</button>{/if}
        <button class:primary={!canPickFolder} disabled={busy} onclick={asZip}>Download ZIP</button>
      {/if}
    </div>
    <ul class="list small">
      {#each files as f (f.path)}
        <li class="row spread">
          <span class="ellipsis" data-testid="received-file">{f.path}</span>
          <span class="row">
            <span class="muted">{bytes(f.size)}</span>
            <button onclick={() => download(f.blob!, basename(f.path))}>Save</button>
          </span>
        </li>
      {/each}
    </ul>
  </div>
{/if}
{#if message}<p class="muted">{message}</p>{/if}

<style>
  pre {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    background: var(--surface-2);
    border-radius: 10px;
    padding: 12px;
    margin: 0;
    max-height: 50vh;
    overflow: auto;
  }
</style>
