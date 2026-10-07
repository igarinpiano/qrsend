<script lang="ts">
  import { onMount } from "svelte";
  import { persist } from "../lib/db";
  import { engine } from "../lib/engine";
  import type { RecvState } from "../lib/engine-types";
  import { bytes } from "../lib/format";
  import { copyText } from "../lib/save";
  import Camera from "./Camera.svelte";
  import Result from "./Result.svelte";

  let { params }: { params: URLSearchParams } = $props();

  let st = $state<RecvState | undefined>();
  let failure = $state("");
  let rate = $state(0);
  let queued: string[] = [];
  let pushing = false;
  let started = performance.now();
  let locked = false;

  const info = $derived(st?.info ?? undefined);
  const result = $derived(st?.result);
  const error = $derived(failure || st?.error || "");
  const active = $derived(!!st && !result && !error);

  function apply(next: RecvState) {
    st = next;
    if (next.info?.session && !locked) {
      locked = true;
      started = performance.now();
    }
    if (next.info) rate = next.info.useful / Math.max((performance.now() - started) / 1000, 0.001);
  }

  onMount(() => {
    persist();
    engine
      .recvStart(params.get("session") ?? undefined)
      .then(apply)
      .catch((e) => (failure = e instanceof Error ? e.message : String(e)));
    return () => {
      engine.recvStop().catch(() => {});
    };
  });

  // Codes arrive faster than they are stored; send them on in batches, one
  // request at a time.
  async function flush() {
    if (pushing) return;
    pushing = true;
    try {
      while (queued.length && !result && !error) {
        const batch = queued;
        queued = [];
        apply(await engine.recvPush(batch));
      }
    } catch (e) {
      failure = e instanceof Error ? e.message : String(e);
    } finally {
      pushing = false;
    }
  }

  function ontexts(texts: string[]) {
    if (!st || result || error) return;
    queued.push(...texts);
    flush();
  }

  const pct = $derived(info && info.total ? (info.done / info.total) * 100 : 0);
</script>

<h2>Receive</h2>

{#if !result}
  <Camera {ontexts} {active} allowFile />
{/if}

{#if error}
  <p class="error" role="alert">{error}</p>
{/if}
{#if st?.notice && !error}
  <p class="muted small">{st.notice}</p>
{/if}
{#if st && !st.persistent}
  <p class="muted small">This browser offers no file storage here (private window?), so received data is kept in memory only.</p>
{/if}

{#if info?.session && !result}
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
        {#if rate > 0} · {rate.toFixed(1)} useful codes/s{/if}
      </span>
    </div>
    {#if st?.resumeCode}
      <details>
        <summary class="small">Missing pieces? Resume code</summary>
        <p class="small">Run this on the sending computer to resend only what is missing:</p>
        <p class="row">
          <code>qrsend send --resume {st.resumeCode}</code>
          <button onclick={() => copyText(st!.resumeCode!)}>Copy</button>
        </p>
        <p class="small muted">Progress is saved — you can leave and continue later from the Inbox.</p>
      </details>
    {/if}
  </div>
{:else if !error && !result}
  <p class="muted">
    Point the camera at the sender’s screen and keep the whole code in view — or pick a video recording of the screen.
  </p>
{/if}

{#if result}
  <Result {result} />
  <p class="small muted">Kept in your <a href="#/inbox">Inbox</a> until you delete it.</p>
{/if}
