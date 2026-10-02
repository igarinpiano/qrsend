<script lang="ts">
  import { basename, canPickFolder, copyText, download, saveToFolder, zip, type FileEntry } from "../lib/save";
  import { bytes } from "../lib/format";

  let { result, name }: { result: { kind: "text"; text: string } | { kind: "files"; entries: FileEntry[] }; name: string } =
    $props();

  let message = $state("");
  const files = $derived(result.kind === "files" ? result.entries.filter((e) => !e.dir) : []);

  async function copy(text: string) {
    await copyText(text);
    message = "Copied to the clipboard.";
  }

  async function toFolder() {
    if (result.kind !== "files") return;
    try {
      const n = await saveToFolder(result.entries);
      message = `Saved ${n} file${n === 1 ? "" : "s"}.`;
    } catch (e) {
      if (!(e instanceof DOMException && e.name === "AbortError")) message = String(e);
    }
  }

  function asZip() {
    if (result.kind !== "files") return;
    download(zip(result.entries), `${name}.zip`);
  }
</script>

{#if result.kind === "text"}
  <div class="card stack">
    <h3>Received text</h3>
    <pre data-testid="received-text">{result.text}</pre>
    <div class="row">
      <button class="primary" onclick={() => copy(result.text)}>Copy</button>
      <button onclick={() => download(new TextEncoder().encode(result.text), `${name}.txt`)}>Save as file</button>
    </div>
  </div>
{:else}
  <div class="card stack">
    <h3>Received {files.length} file{files.length === 1 ? "" : "s"}</h3>
    <div class="row">
      {#if files.length === 1 && !files[0].path.includes("/")}
        <button class="primary" onclick={() => download(files[0].data!, basename(files[0].path))}>Save {basename(files[0].path)}</button>
      {:else}
        {#if canPickFolder}<button class="primary" onclick={toFolder}>Save to folder…</button>{/if}
        <button class:primary={!canPickFolder} onclick={asZip}>Download ZIP</button>
      {/if}
    </div>
    <ul class="list small">
      {#each files as f (f.path)}
        <li class="row spread">
          <span class="ellipsis" data-testid="received-file">{f.path}</span>
          <span class="row">
            <span class="muted">{bytes(f.data!.length)}</span>
            <button onclick={() => download(f.data!, basename(f.path))}>Save</button>
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
