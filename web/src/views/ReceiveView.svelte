<script lang="ts">
  import { onMount } from "svelte";
  import type { Receive } from "../lib/core";
  import { persist } from "../lib/db";
  import { bytes, duration } from "../lib/format";
  import { getSession, newReceive, restore, saveRecord, saveSegment, type SessionRecord } from "../lib/inbox";
  import { copyText, type FileEntry } from "../lib/save";
  import Camera from "./Camera.svelte";
  import Result from "./Result.svelte";

  let { params }: { params: URLSearchParams } = $props();

  interface Info {
    session: string | null;
    encrypted: boolean;
    total: number;
    done: number;
    complete: boolean;
    kind: "text" | "files" | null;
    summary: string | null;
    plain_length: number | null;
    wire_length: number | null;
    sender_name: string | null;
    sender_status: "trusted" | "unverified" | "unsigned" | null;
    sender: string | null;
    entries: { path: string; dir: boolean; size: number }[];
    useful: number;
  }

  interface PushResult {
    locked: string | null;
    foreign: string | null;
    completed: number[];
    meta: boolean;
    rejected: number[];
    error: string | null;
  }

  let r: Receive | undefined;
  let info = $state<Info | undefined>();
  let error = $state("");
  let notice = $state("");
  let active = $state(true);
  let result = $state<{ kind: "text"; text: string } | { kind: "files"; entries: FileEntry[] } | undefined>();
  let resumeCode = $state<string | undefined>();
  let record: SessionRecord | undefined;
  let queue: Promise<void> = Promise.resolve();
  let started = performance.now();
  let rate = $state(0);

  onMount(async () => {
    persist();
    const session = params.get("session");
    try {
      r = session ? await restore(session) : await newReceive();
      if (session) record = await getSession(session);
      refresh();
      if (r.isComplete()) finish();
    } catch (e) {
      error = String(e);
    }
  });

  function refresh() {
    if (!r) return;
    info = r.info() as Info;
    resumeCode = r.resumeCode() ?? undefined;
    const secs = (performance.now() - started) / 1000;
    rate = info.useful / Math.max(secs, 0.001);
  }

  function ontexts(texts: string[]) {
    if (!r || result || error) return;
    for (const text of texts) {
      const res = r.push(text) as PushResult;
      if (res.foreign) notice = `Ignoring codes of another transfer (${res.foreign}).`;
      if (res.locked) {
        const p = r.params() as { session: string; flags: number; segShift: number; segCount: number };
        record = { ...p, created: Date.now(), updated: Date.now(), done: 0, total: p.segCount + 1, complete: false };
        started = performance.now();
      }
      if (res.error) {
        error = res.error;
        active = false;
      }
      if (res.rejected.length) notice = `${res.rejected.length} segment(s) failed verification and will be received again.`;
    }
    const completed = r.takeCompleted() as { index: number; data: Uint8Array }[];
    refresh();
    if (record && info) {
      const rec = record;
      rec.done = info.done;
      rec.summary = info.summary ?? undefined;
      const snapshot = { ...rec };
      queue = queue.then(async () => {
        for (const c of completed) await saveSegment(rec.session, c.index, c.data);
        await saveRecord(snapshot);
      });
    }
    if (r.isComplete()) finish();
  }

  async function finish() {
    if (!r || result) return;
    active = false;
    try {
      result = r.extract() as typeof result;
      if (record) {
        record.complete = true;
        record.done = record.total;
        const rec = { ...record };
        queue = queue.then(() => saveRecord(rec));
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  const pct = $derived(info && info.total ? (info.done / info.total) * 100 : 0);
  const name = $derived(info?.session ? `qrsend-${info.session}` : "qrsend");
</script>

<h2>Receive</h2>

{#if !result}
  <Camera {ontexts} {active} />
{/if}

{#if error}
  <p class="error" role="alert">{error}</p>
{/if}
{#if notice && !error}
  <p class="muted small">{notice}</p>
{/if}

{#if info?.session}
  <div class="card stack">
    <div class="row spread">
      <strong class="ellipsis">{info.summary ?? "Waiting for the file list…"}</strong>
      <span class="badge">{info.session}</span>
    </div>
    {#if info.sender_status}
      <div class="row small">
        <span>From:</span>
        {#if info.sender_status === "trusted"}
          <span class="badge ok">{info.sender} ✓</span>
        {:else if info.sender_status === "unverified"}
          <span class="badge warn">Unverified sender</span>
          <span class="muted">{info.sender_name ? `claims to be “${info.sender_name}”` : ""} key {info.sender}</span>
        {:else}
          <span class="badge warn">Unsigned</span>
          {#if info.sender_name}<span class="muted">claims to be “{info.sender_name}”</span>{/if}
        {/if}
        {#if info.encrypted}<span class="badge ok">Encrypted for this device</span>{/if}
      </div>
    {/if}
    <div class="progress" aria-label="Progress"><div style:width="{pct}%"></div></div>
    <div class="row spread small muted">
      <span>{info.done} / {info.total} segments</span>
      <span>
        {#if info.wire_length}{bytes(info.wire_length)}{/if}
        {#if rate > 0 && !result} · {rate.toFixed(1)} useful codes/s{/if}
      </span>
    </div>
    {#if resumeCode && !result}
      <details>
        <summary class="small">Missing pieces? Resume code</summary>
        <p class="small">Run this on the sending computer to resend only what is missing:</p>
        <p class="row">
          <code>qrsend send --resume {resumeCode}</code>
          <button onclick={() => copyText(resumeCode!)}>Copy</button>
        </p>
        <p class="small muted">Progress is saved — you can leave and continue later from the Inbox.</p>
      </details>
    {/if}
  </div>
{:else if !error}
  <p class="muted">Point the camera at the sender’s screen. Keep the whole code in view; distance matters more than focus.</p>
{/if}

{#if result}
  <Result {result} {name} />
  <p class="small muted">Kept in your <a href="#/inbox">Inbox</a> until you delete it.</p>
{/if}

{#if info && !info.session && !error}
  <p class="small muted">Estimated speed appears once codes are detected.{rate > 0 ? ` ${duration(0)}` : ""}</p>
{/if}
