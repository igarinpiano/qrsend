<script lang="ts">
  import { onMount } from "svelte";
  import { estimate } from "../lib/db";
  import { engine } from "../lib/engine";
  import type { RecvResult, SessionRecord } from "../lib/engine-types";
  import { bytes } from "../lib/format";
  import { copyText } from "../lib/save";
  import Result from "./Result.svelte";

  let sessions = $state<SessionRecord[]>([]);
  let storage = $state<{ usage: number; quota: number } | undefined>();
  let opened = $state<RecvResult | undefined>();
  let message = $state("");

  async function load() {
    sessions = await engine.inboxList();
    storage = await estimate();
  }
  onMount(load);

  async function open(s: SessionRecord) {
    message = "";
    try {
      opened = await engine.inboxOpen(s.session);
      await load();
    } catch (e) {
      message = e instanceof Error ? e.message : String(e);
    }
  }

  async function resume(s: SessionRecord) {
    if (!s.resumeCode) return;
    await copyText(`qrsend send --resume ${s.resumeCode}`);
    message = "Resume command copied.";
  }

  async function remove(s: SessionRecord) {
    if (!confirm(`Delete ${s.summary ?? s.session}?`)) return;
    await engine.inboxRemove(s.session);
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
          <span class="badge" class:ok={s.complete}>{s.complete ? "Complete" : `${s.doneCount}/${s.total}`}</span>
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
            {#if s.resumeCode}<button onclick={() => resume(s)}>Copy resume command</button>{/if}
          {/if}
          <button class="danger" onclick={() => remove(s)}>Delete</button>
        </div>
      </li>
    {/each}
  </ul>
{/if}

{#if message}<p class="muted">{message}</p>{/if}

{#if opened}
  {#key opened.session}
    <Result result={opened} />
  {/key}
{/if}

{#if storage}
  <p class="small muted">Browser storage used: {bytes(storage.usage)} of {bytes(storage.quota)}</p>
{/if}
