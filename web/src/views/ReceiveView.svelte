<script lang="ts">
  import { onMount } from "svelte";
  import { ready, renderText } from "../lib/core";
  import { persist } from "../lib/db";
  import { engine } from "../lib/engine";
  import type { RecvState } from "../lib/engine-types";
  import { bytes, duration, RateMeter } from "../lib/format";
  import { identityInfo, loadIdentity, sign } from "../lib/keys";
  import {
    LINK_PREFIX,
    LanReceiver,
    LinkAssembler,
    canConnect,
    fingerprintHex,
    introductionLine,
    introductionMessage,
    isKnownOfferFor,
    isOffer,
    type LinkMessage,
  } from "../lib/lan";
  import { lastingCertificate } from "../lib/linkid";
  import { log, logEvery } from "../lib/log";
  import { featureOn } from "../lib/prefs";
  import { recoverFromLoadFailure } from "../lib/update";
  import { toDataUrl } from "../lib/qrdraw";
  import { feedbackWav } from "../lib/sound";
  import { copyText } from "../lib/save";
  import Camera from "./Camera.svelte";
  import Result from "./Result.svelte";

  let { params }: { params: URLSearchParams } = $props();

  let st = $state<RecvState | undefined>();
  let failure = $state("");
  let rate = $state(0);
  let queued: string[] = [];
  /** What the camera makes of the codes, for a sender that adjusts to it. */
  let camera: { reads: number; dot: number } | undefined;
  let pushing = false;
  const meter = new RateMeter();

  // Measurements (a preview feature): how fast received codes are taken in.
  const showStats = featureOn("stats");
  let intake = $state({ msPerBatch: 0, codesPerBatch: 0, waiting: 0, linkRate: 0 });
  let linkBytes = 0;
  let linkBytesAt = 0;
  let linkBytesSeen = 0;
  // When each step toward the transfer happened, and what kinds of codes
  // were read: for finding out which step a slow start is waiting for.
  type Step = "first" | "list" | "offer" | "answer" | "connected";
  let steps = $state<Partial<Record<Step, number>>>({});
  let seen = $state({ codes: 0, notices: 0, offers: 0 });
  function mark(step: Step) {
    if (steps[step] !== undefined) return;
    steps[step] = performance.now();
    log("rx", `step: ${step}`, { sinceFirstCode: step === "first" ? undefined : (steps[step]! - (steps.first ?? steps[step]!)) / 1000 });
  }
  const after = (at: number | undefined) =>
    at === undefined || steps.first === undefined ? "not yet" : `after ${((at - steps.first) / 1000).toFixed(1)} s`;

  // When the sender asks for it (two-way transfer), this screen shows it what
  // is still missing, so it sends only that and stops by itself. The code is
  // redrawn a few times per second at most, so a camera gets a steady look.
  const FEEDBACK_HOLD_MS = 300;
  let feedback = $state("");
  let feedbackUrl = $state("");
  let feedbackAt = 0;
  let coreReady = false;
  ready()
    .then(() => {
      coreReady = true;
      if (st) showFeedback(st, true);
    })
    .catch(() => recoverFromLoadFailure());

  // A sender that listens with a microphone can be answered by sound: the
  // feedback code as a second of chirps from this device's speaker. Nothing
  // sounds unless the person receiving allows it.
  const SOUND_EVERY_MS = 2500;
  let soundChoice = $state<"ask" | "yes" | "no">("ask");
  let speaker = $state<HTMLAudioElement | undefined>();
  let soundUrl = $state("");
  let soundCode = $state("");
  let soundAt = 0;
  let sounding = false;

  /** Plays the latest feedback code, unless one is playing or was played a moment ago. */
  async function sound(force = false) {
    const code = st?.feedback;
    if (soundChoice !== "yes" || !speaker || !code || sounding) return;
    const now = performance.now();
    if (!force && (code === soundCode || now - soundAt < SOUND_EVERY_MS)) return;
    sounding = true;
    soundAt = now;
    if (code !== soundCode) {
      const wav = await feedbackWav(code);
      if (!wav || !speaker) {
        sounding = false;
        return;
      }
      if (soundUrl) URL.revokeObjectURL(soundUrl);
      soundUrl = URL.createObjectURL(wav);
      speaker.src = soundUrl;
      soundCode = code;
    } else {
      speaker.currentTime = 0;
    }
    speaker.onended = speaker.onerror = () => {
      sounding = false;
      // The last word must get through: say "everything arrived" a few times.
      if (st?.result && completeSaid < 3) {
        completeSaid++;
        setTimeout(() => sound(true), 400);
      }
    };
    speaker.play().catch(() => (sounding = false));
  }
  let completeSaid = 0;

  function allowSound() {
    log("rx", "sound allowed");
    soundChoice = "yes";
    // Started by this tap, so the browser lets the page play from now on.
    sound(true);
  }

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
  // A short transfer can be over before the sender's first notice has been
  // read. The camera then stays on a little longer, only to learn whether the
  // sender wants to be told that everything arrived.
  const LINGER_MS = 8000;
  let lingering = $state(false);
  let lingerTimer: ReturnType<typeof setTimeout> | undefined;
  const watching = $derived(!result || lingering);
  const active = $derived(!!st && !error && watching);

  // The sender may offer a direct connection over the local network (its
  // "Local network boost"). The offer is accepted as it comes: the answer goes
  // back as a code on this screen, and the codes arriving through the
  // connection join those from the camera. The person receiving can end the
  // connection, and it is not taken up again in this transfer.
  // Replies through the connection are gathered into one message every so
  // often. (A small reply for every message received makes a browser's data
  // channel crawl: measured 0.6 MB/s instead of 15.)
  const REPLY_EVERY_MS = 100;
  const assembler = new LinkAssembler();
  let offer = $state<LinkMessage | undefined>();
  let linkDeclined = $state(false);
  let linkState = $state<"none" | "answering" | "connected" | "closed">("none");
  let answerUrl = $state("");
  let linkError = $state("");
  /** Answering the usual offer (with a code on this screen), and taking up an offer made to this device. */
  let lan: LanReceiver | undefined;
  let silent: LanReceiver | undefined;
  /** Whichever of the two is connected. */
  let linked: LanReceiver | undefined;
  let linkCodes: string[] = [];
  let linkPacked: ArrayBuffer[] = [];
  /** Messages handed to the engine in one go: about 2 MiB of data. */
  const PACKED_PER_PUSH = 32;
  let linkTaken = 0;
  let linkAcked = 0;
  let feedbackSent = "";
  let feedbackSentAt = 0;
  const FEEDBACK_AGAIN_MS = 1000;
  let replyTimer: ReturnType<typeof setInterval> | undefined;

  /** Tells the sender how far we are: codes taken in ("A<n>") and the latest feedback. */
  function reply() {
    if (!linked?.connected) return;
    if (linked.lasting && !introducedSelf && info?.sender_status === "trusted") introduce(linked);
    const lines: string[] = [];
    if (linkTaken > linkAcked) {
      linkAcked = linkTaken;
      lines.push(`A${linkTaken}`);
    }
    // Said again once a second while nothing changes (a large transfer takes
    // a while to unpack): the sender must not take silence for a lost receiver.
    const now = performance.now();
    if (st?.feedback && (st.feedback !== feedbackSent || now - feedbackSentAt > FEEDBACK_AGAIN_MS)) {
      feedbackSent = st.feedback;
      feedbackSentAt = now;
      lines.push(st.feedback);
    }
    if (lines.length) linked.send(lines.join("\n"));
  }

  // A sender that remembers the devices it trusts (its offer says so) is
  // told which device this is, once the transfer has shown that the sender
  // is a device this one trusts in turn: signed with this device's key, and
  // naming the certificate this connection uses. Next time that sender can
  // connect without this screen being shown to it.
  let introducedSelf = false;
  async function introduce(to: LanReceiver) {
    introducedSelf = true;
    log("rx", "introducing this device to a trusted sender");
    try {
      const me = await loadIdentity();
      const mine = await lastingCertificate(false);
      if (!me || !mine) return;
      const certificate = fingerprintHex(mine.fingerprint);
      const { signature } = await sign(me, introductionMessage(certificate));
      to.send(introductionLine(identityInfo(me).id, certificate, btoa(String.fromCharCode(...signature))));
    } catch (e) {
      console.warn("introduction:", e);
    }
  }

  // The offer made to this device in particular by a sender that remembers
  // it: taken up without showing anything. (The usual offer is answered as
  // well; whichever connects first is used.)
  let knownTried: number | undefined;
  async function knownOffer(msg: LinkMessage) {
    if (linkDeclined || linked || knownTried === msg.id) return;
    try {
      const mine = await lastingCertificate(false);
      if (!mine || !(await isKnownOfferFor(msg, mine.fingerprint)) || knownTried === msg.id) return;
      knownTried = msg.id;
      log("rx", "offer to this device", { id: msg.id });
      silent ??= receiver();
      await silent.acceptKnown(msg, mine.certificate);
    } catch (e) {
      console.warn("offer to this device:", e);
    }
  }

  /** A connection in the making; it reports here when it is up or gone. */
  function receiver(): LanReceiver {
    const made: LanReceiver = new LanReceiver({
      codes: (codes) => {
        linkCodes.push(...codes);
        flush();
      },
      packed: (message) => {
        linkPacked.push(message);
        flush();
      },
      state: (state) => {
        if (state === "closed") {
          // An attempt that never connected changes nothing.
          if (linked !== made) return;
          linked = undefined;
        }
        linkState = state;
        clearInterval(replyTimer);
        if (state === "connected") {
          mark("connected");
          linked = made;
          log("rx", "connected", { by: made === silent ? "offer to this device" : "answer code", lasting: made.lasting });
          for (const other of [lan, silent]) if (other !== made) other?.stop();
          introducedSelf = false;
          // Counts start over with every connection (the sender's do), and
          // the sender starts again from what is missing.
          linkCodes = [];
          linkPacked = [];
          linkTaken = linkAcked = 0;
          feedbackSent = "";
          answerUrl = "";
          replyTimer = setInterval(reply, REPLY_EVERY_MS);
        }
      },
    });
    return made;
  }

  function onLinkCode(code: string) {
    const msg = assembler.add(code);
    if (!msg || !canConnect || msg.session !== info?.session) return;
    if (!isOffer(msg)) {
      void knownOffer(msg);
      return;
    }
    mark("offer");
    if (offer && offer.id === msg.id) return;
    offer = msg;
    connect();
  }

  function disconnect() {
    log("rx", "disconnected by hand");
    linkDeclined = true;
    answerUrl = "";
    linkState = "none";
    clearInterval(replyTimer);
    lan?.stop();
    silent?.stop();
    linked = undefined;
  }

  async function connect() {
    linkError = "";
    if (!offer || linkDeclined) return;
    lan ??= receiver();
    try {
      linkState = "answering";
      const answer = await lan.accept(offer, async () => (await lastingCertificate(true))?.certificate);
      // Connected meanwhile, by the offer made to this device: nothing to show.
      if (linked) return;
      await ready();
      answerUrl = toDataUrl(renderText(answer), 5);
      mark("answer");
    } catch (e) {
      linkState = "closed";
      linkError = e instanceof Error ? e.message : String(e);
      log("rx", "answering failed", { error: linkError.slice(0, 200) });
    }
  }

  function apply(next: RecvState) {
    if (next.result && !st?.result && !next.feedback) {
      lingering = true;
      lingerTimer = setTimeout(() => (lingering = false), LINGER_MS);
    }
    if (next.result && next.feedback) lingering = false;
    if (next.error && next.error !== st?.error) log("rx", "error", { message: next.error.slice(0, 200) });
    if (next.feedback && !st?.feedback) log("rx", "sender asked for feedback", { bySound: next.feedbackBySound });
    if (next.result && !st?.result) log("rx", "done", { kind: next.result.kind });
    st = next;
    if (next.info?.summary) mark("list");
    logEvery("rx", 2000, "rx", "progress", () => ({
      bytes: next.info?.total_bytes != null ? next.info.total_bytes - (next.info.remaining_bytes ?? 0) : undefined,
      of: next.info?.total_bytes,
      codes: next.info?.total_codes != null ? next.info.total_codes - (next.info.remaining_codes ?? 0) : undefined,
      ofCodes: next.info?.total_codes,
      fileList: next.info?.summary ? "here" : `${next.info?.list_have ?? 0}/${next.info?.list_need ?? "?"}`,
      perS: Math.round(rate),
      read: seen.codes,
      notices: seen.notices,
      offers: seen.offers,
      link: linkState,
      linkWaiting: linkCodes.length + linkPacked.length * 16,
      msPerBatch: intake.msPerBatch,
      linkPerS: Math.round(intake.linkRate),
    }));
    showFeedback(next);
    if (next.feedbackBySound) sound(!!next.result && completeSaid === 0);
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
      .catch((e) => {
        failure = e instanceof Error ? e.message : String(e);
        log("rx", "could not start", { error: failure.slice(0, 200) });
      });
    return () => {
      clearTimeout(lingerTimer);
      clearInterval(replyTimer);
      lan?.stop();
      silent?.stop();
      engine.recvStop().catch(() => {});
    };
  });

  // Codes arrive faster than they are stored; send them on in batches, one
  // request at a time.
  async function flush() {
    if (pushing) return;
    pushing = true;
    try {
      while ((queued.length || linkCodes.length || linkPacked.length) && !result && !error) {
        // A connection delivers much faster than it can be taken in: take it
        // in pieces, and tell the sender how far we are.
        const fromLink = linkCodes.splice(0, 64);
        const packed = linkPacked.splice(0, PACKED_PER_PUSH);
        const packedBytes = packed.reduce((n, m) => n + m.byteLength, 0);
        const batch = queued.concat(fromLink);
        queued = [];
        const pushedAt = performance.now();
        const next = await engine.recvPush(batch, packed, camera);
        linkTaken += fromLink.length + (next.taken ?? 0);
        apply(next);
        {
          const now = performance.now();
          const ease = (average: number, value: number) => (average === 0 ? value : average + (value - average) / 20);
          // Codes are Base45 text: three characters carry two bytes.
          for (const code of fromLink) linkBytes += (code.length * 2) / 3;
          linkBytes += packedBytes;
          if (now - linkBytesAt > 1000) {
            if (linkBytesAt) intake.linkRate = ((linkBytes - linkBytesSeen) * 1000) / (now - linkBytesAt);
            linkBytesAt = now;
            linkBytesSeen = linkBytes;
          }
          intake.msPerBatch = ease(intake.msPerBatch, now - pushedAt);
          intake.codesPerBatch = ease(intake.codesPerBatch, batch.length + (next.taken ?? 0));
          intake.waiting = linkCodes.length + linkPacked.length * 16;
        }
      }
    } catch (e) {
      failure = e instanceof Error ? e.message : String(e);
      log("rx", "failed", { error: failure.slice(0, 200) });
    } finally {
      pushing = false;
    }
  }

  function ontexts(texts: string[]) {
    if (!st || error) return;
    mark("first");
    seen.codes += texts.length;
    seen.notices += texts.filter((t) => t.startsWith("QSC1-")).length;
    seen.offers += texts.filter((t) => t.startsWith(LINK_PREFIX)).length;
    if (result) {
      // Everything is here, but a sender that asks for feedback only now
      // (a short transfer can be over before its first notice) must still be
      // told so.
      const notices = feedback ? [] : texts.filter((t) => t.startsWith("QSC1-"));
      if (notices.length) engine.recvPush(notices).then(apply).catch(() => {});
      return;
    }
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

{#if watching}
  <Camera {ontexts} oncamera={(reads, dot) => (camera = { reads, dot })} {active} allowFile={!result} />
{/if}

{#if showStats && steps.first !== undefined}
  <p class="small muted" data-testid="rx-steps">
    Since the first code: file list {after(steps.list)} · offer to connect read {after(steps.offer)} · answer shown
    {after(steps.answer)} · connected {after(steps.connected)}. Of {seen.codes} codes read, {seen.notices} were notices and
    {seen.offers} offers.
  </p>
{/if}
{#if showStats && intake.msPerBatch > 0 && !result}
  <p class="small muted" data-testid="rx-stats">
    Taking in: {intake.msPerBatch.toFixed(1)} ms per batch of {intake.codesPerBatch.toFixed(0)} pieces · {intake.waiting} waiting
    {#if intake.linkRate > 0}· network {bytes(Math.round(intake.linkRate))}/s{/if}
  </p>
{/if}

{#if st?.feedbackBySound && soundChoice === "ask"}
  <div class="card stack" data-testid="sound-offer">
    <p>
      <strong>The sender listens for feedback by sound.</strong> This device would answer with short runs of soft notes
      from its speaker, so the sender sends only what is missing and stops when everything has arrived.
    </p>
    <div class="row">
      <button class="primary" onclick={allowSound}>Answer by sound</button>
      <button onclick={() => (soundChoice = "no")}>Stay silent</button>
    </div>
  </div>
{/if}
{#if soundChoice === "yes"}
  <p class="small muted"><span class="badge ok">Sound</span> Answering the sender by sound. Keep the devices close.</p>
{/if}
<audio bind:this={speaker} data-testid="feedback-sound" data-code={soundCode}></audio>

{#if answerUrl && !result}
  <div class="card feedback">
    <img src={answerUrl} alt="Connection code for the sender" data-testid="link-answer" />
    <div class="stack">
      <p class="small muted">
        The sender offers a direct connection over the local network, much faster than the camera. Show this code to the
        sender’s camera to connect; after that the devices no longer need to see each other.
      </p>
      <button onclick={disconnect}>Use the camera only</button>
    </div>
  </div>
{/if}
{#if linkState === "connected" && !result}
  <p class="small row" data-testid="link-connected">
    <span class="badge ok">Local network</span>
    <span>Receiving directly from the sender, and through the camera as well.</span>
    <button onclick={disconnect}>Disconnect</button>
  </p>
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
      <strong class="ellipsis" data-testid="summary">
        {info.summary ?? `Waiting for the file list…${info.list_need ? ` ${info.list_have} of ${info.list_need} codes` : ""}`}
      </strong>
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
