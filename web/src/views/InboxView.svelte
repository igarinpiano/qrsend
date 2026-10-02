<script lang="ts">
  import { onMount } from "svelte";
  import { estimate } from "../lib/db";
  import { bytes } from "../lib/format";
  import { deleteSession, listSessions, restore, type SessionRecord } from "../lib/inbox";
  import { copyText, type FileEntry } from "../lib/save";
  import Result from "./Result.svelte";

  let sessions = $state<SessionRecord[]>([]);
  let storage = $state<{ usage: number; quota: number } | undefined>();
  let opened = $state<{ session: string; result: { kind: "text"; text: string } | { kind: "files"; entries: FileEntry[] } } | undefined>();
  let error = $state("");

  async function load() {
    sessions = await listSessions();
    storage = await estimate();
  }
  onMount(load);

  async function open(s: SessionRecord) {
    error = "";
    try {
      const r = await restore(s.session);
      opened = { session: s.session, result: r.extract() as never };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function resume(s: SessionRecord) {
    const r = await restore(s.session);
    const code = r.resumeCode();
    if (code) await copyText(`qrsend send --resume ${code}`);
    error = code ? "Resume command copied." : "";
  }

  async function remove(s: SessionRecord) {
    if (!confirm(`Delete ${s.summary ?? s.session}?`)) return;
    await deleteSession(s.session);
    if (opened?.session === s.session) opened = undefined;
    await load();
  }
</script>

<h2>Inbox</h2>

{#if sessions.length === 0}
  <p class="muted">Nothing received yet.</p>
{:else}
  <ul class="list card">
    {#each sessions as s (s.session)}
      <li class="stack">
        <div class="row spread">
          <strong class="ellipsis">{s.summary ?? "Unknown transfer"}</strong>
          <span class="badge" class:ok={s.complete}>{s.complete ? "Complete" : `${s.done}/${s.total}`}</span>
        </div>
        <div class="row small muted">
          <span class="mono">{s.session}</span>
          <span>{new Date(s.updated).toLocaleString()}</span>
        </div>
        <div class="row">
          {#if s.complete}
            <button class="primary" onclick={() => open(s)}>Open</button>
          {:else}
            <a class="button primary" href="#/receive?session={s.session}">Continue</a>
            <button onclick={() => resume(s)}>Copy resume command</button>
          {/if}
          <button class="danger" onclick={() => remove(s)}>Delete</button>
        </div>
      </li>
    {/each}
  </ul>
{/if}

{#if error}<p class="muted">{error}</p>{/if}

{#if opened}
  <Result result={opened.result} name={`qrsend-${opened.session}`} />
{/if}

{#if storage}
  <p class="small muted">Browser storage used: {bytes(storage.usage)} of {bytes(storage.quota)}</p>
{/if}
