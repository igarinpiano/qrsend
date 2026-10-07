<script lang="ts">
  import { onMount } from "svelte";
  import { ready, renderText } from "../lib/core";
  import { persist } from "../lib/db";
  import { engine } from "../lib/engine";
  import type { RecvState } from "../lib/engine-types";
  import { bytes, duration, RateMeter } from "../lib/format";
  import { LINK_PREFIX, LanReceiver, LinkAssembler, canConnect, isOffer, type LinkMessage } from "../lib/lan";
  import { toDataUrl } from "../lib/qrdraw";
  import { copyText } from "../lib/save";
  import Camera from "./Camera.svelte";
  import Result from "./Result.svelte";

  let { params }: { params: URLSearchParams } = $props();

  let st = $state<RecvState | undefined>();
  let failure = $state("");
  let rate = $state(0);
  let queued: string[] = [];
  let pushing = false;
  const meter = new RateMeter();

  // When the sender asks for it (two-way transfer), this screen shows it what
  // is still missing, so it sends only that and stops by itself. The code is
  // redrawn a few times per second at most, so a camera gets a steady look.
  const FEEDBACK_HOLD_MS = 300;
  let feedback = $state("");
  let feedbackUrl = $state("");
  let feedbackAt = 0;
  let coreReady = false;
  ready().then(() => {
    coreReady = true;
    if (st) showFeedback(st, true);
  });

  function showFeedback(next: RecvState, force = false) {
    if (!coreReady || !next.feedback) return;
    const now = performance.now();
    if (!force && !next.result && now - feedbackAt < FEEDBACK_HOLD_MS) return;
    feedbackAt = now;
    feedback = next.feedback;
    feedbackUrl = toDataUrl(renderText(feedback), 6);
  }


  const info = $derived(st?.info ?? undefined);
  const result = $derived(st?.result);
  const error = $derived(failure || st?.error || "");
  const active = $derived(!!st && !result && !error);

  // The sender may offer a direct connection over the local network (its
  // "Local network boost"). Nothing connects unless the person receiving
  // agrees; the answer then goes back as a code on this screen, and the codes
  // arriving through the connection join those from the camera.
  // Replies through the connection are gathered into one message every so
  // often. (A small reply for every message received makes a browser's data
  // channel crawl: measured 0.6 MB/s instead of 15.)
  const REPLY_EVERY_MS = 100;
  const assembler = new LinkAssembler();
  let offer = $state<LinkMessage | undefined>();
  let linkChoice = $state<"ask" | "yes" | "no">("ask");
  let linkState = $state<"none" | "answering" | "connected" | "closed">("none");
  let answerUrl = $state("");
  let linkError = $state("");
  let lan: LanReceiver | undefined;
  let linkCodes: string[] = [];
  let linkTaken = 0;
  let linkAcked = 0;
  let feedbackSent = "";
  let replyTimer: ReturnType<typeof setInterval> | undefined;

  /** Tells the sender how far we are: codes taken in ("A<n>") and the latest feedback. */
  function reply() {
    if (!lan?.connected) return;
    const lines: string[] = [];
    if (linkTaken !== linkAcked) {
      linkAcked = linkTaken;
      lines.push(`A${linkTaken}`);
    }
    if (st?.feedback && st.feedback !== feedbackSent) {
      feedbackSent = st.feedback;
      lines.push(st.feedback);
    }
    if (lines.length) lan.send(lines.join("\n"));
  }

  function onLinkCode(code: string) {
    const msg = assembler.add(code);
    if (!msg || !isOffer(msg) || !canConnect || msg.session !== info?.session) return;
    if (offer && offer.id === msg.id) return;
    offer = msg;
    if (linkChoice === "yes") connect();
  }

  async function connect() {
    linkChoice = "yes";
    linkError = "";
    if (!offer) return;
    lan ??= new LanReceiver({
      codes: (codes) => {
        linkCodes.push(...codes);
        flush();
      },
      state: (state) => {
        linkState = state;
        clearInterval(replyTimer);
        if (state === "connected") {
          answerUrl = "";
          replyTimer = setInterval(reply, REPLY_EVERY_MS);
        }
      },
    });
    try {
      linkState = "answering";
      const answer = await lan.accept(offer);
      await ready();
      answerUrl = toDataUrl(renderText(answer), 5);
    } catch (e) {
      linkState = "closed";
      linkError = e instanceof Error ? e.message : String(e);
    }
  }

  function apply(next: RecvState) {
    st = next;
    showFeedback(next);
    // The sender should hear at once that everything has arrived.
    if (next.result) reply();
    // What matters while waiting: how much is left and how fast it goes.
    if (next.info?.remaining_bytes != null) rate = meter.update(performance.now(), next.info.remaining_bytes);
  }

  onMount(() => {
    persist();
    engine
      .recvStart(params.get("session") ?? undefined)
      .then(apply)
      .catch((e) => (failure = e instanceof Error ? e.message : String(e)));
    return () => {
      clearInterval(replyTimer);
      lan?.stop();
      engine.recvStop().catch(() => {});
    };
  });

  // Codes arrive faster than they are stored; send them on in batches, one
  // request at a time.
  async function flush() {
    if (pushing) return;
    pushing = true;
    try {
      while ((queued.length || linkCodes.length) && !result && !error) {
        // A connection delivers codes much faster than they can be taken in:
        // take them in pieces, and tell the sender how far we are.
        const fromLink = linkCodes.splice(0, 64);
        const batch = queued.concat(fromLink);
        queued = [];
        apply(await engine.recvPush(batch));
        linkTaken += fromLink.length;
      }
    } catch (e) {
      failure = e instanceof Error ? e.message : String(e);
    } finally {
      pushing = false;
    }
  }

  function ontexts(texts: string[]) {
    if (!st || result || error) return;
    for (const t of texts) {
      if (t.startsWith(LINK_PREFIX)) onLinkCode(t);
      else queued.push(t);
    }
    flush();
  }

  const total = $derived(info?.total_bytes ?? 0);
  const left = $derived(info?.remaining_bytes ?? 0);
  const pct = $derived(total ? ((total - left) / total) * 100 : 0);
</script>

<h2>Receive</h2>

{#if !result}
  <Camera {ontexts} {active} allowFile />
{/if}

{#if offer && !result && linkChoice === "ask"}
  <div class="card stack" data-testid="link-offer">
    <p>
      <strong>The sender offers a direct connection over the local network.</strong> It is much faster than the camera. Both
      devices must be on the same network; nothing outside it is contacted.
    </p>
    <div class="row">
      <button class="primary" onclick={connect}>Connect</button>
      <button onclick={() => (linkChoice = "no")}>No, keep using the camera</button>
    </div>
  </div>
{/if}
{#if answerUrl && !result}
  <div class="card feedback">
    <img src={answerUrl} alt="Connection code for the sender" data-testid="link-answer" />
    <p class="small muted">Show this code to the sender’s camera to connect. After that the devices no longer need to see each other.</p>
  </div>
{/if}
{#if linkState === "connected" && !result}
  <p class="small" data-testid="link-connected"><span class="badge ok">Local network</span> Receiving directly from the sender, and through the camera as well.</p>
{:else if linkError}
  <p class="small muted">No direct connection ({linkError}); the camera carries on.</p>
{/if}

{#if feedbackUrl && linkState !== "answering"}
  <div class="card feedback">
    <img src={feedbackUrl} alt="Feedback code for the sender" data-testid="feedback" data-code={feedback} />
    <p class="small muted">
      {#if result}
        Show this to the sender’s camera once more so it knows everything arrived.
      {:else}
        The sender asked for feedback. Keep this code in view of its camera: it then sends only what is missing and
        stops when everything has arrived.
      {/if}
    </p>
  </div>
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
      <span data-testid="remaining">
        {#if total}
          {pct.toFixed(0)}% · {bytes(total - left)} of {bytes(total)} · {(info.total_codes ?? 0) - (info.remaining_codes ?? 0)} of {info.total_codes}
          codes, {info.remaining_codes} to go
        {/if}
      </span>
      <span>
        {#if rate > 0}{bytes(Math.round(rate))}/s · {duration(left / rate)} left{/if}
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

<style>
  .feedback {
    display: flex;
    gap: 16px;
    align-items: center;
    flex-wrap: wrap;
  }
  .feedback img {
    width: min(240px, 100%);
    image-rendering: pixelated;
    border-radius: 8px;
    background: #fff;
  }
  .feedback p {
    flex: 1;
    min-width: 12em;
    margin: 0;
  }
</style>
