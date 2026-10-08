<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { engine } from "../lib/engine";
  import type { FrameBatch, ReceiverReport, SendStarted } from "../lib/engine-types";
  import { drawColorGrid, drawGrid, fitGrid } from "../lib/qrdraw";
  import { bytes, duration, RateMeter } from "../lib/format";
  import { colorsWorthShowing, parseDeviceId, ready, verifyDeviceSignature } from "../lib/core";
  import { rememberLink, trustedDevices } from "../lib/devices";
  import { KIND_SEEDED_OFFER, LINK_PREFIX, LanSender, canConnect, fingerprintHex, introductionMessage, type LinkState } from "../lib/lan";
  import { log, logEvery } from "../lib/log";
  import { featureOn } from "../lib/prefs";
  import { Scanner, type ScanStats } from "../lib/scanner";
  import { Ear, canListen } from "../lib/sound";

  /** `grid` is codes per side, or 0 to fill the screen. */
  let { info, fps: initialFps, grid, onclose }: { info: SendStarted; fps: number; grid: number; onclose: () => void } =
    $props();

  // Smallest module size (device pixels) an automatic grid may use.
  const MIN_MODULE_PX = 3;

  let canvas: HTMLCanvasElement;
  let root: HTMLDivElement;
  let fps = $state(untrack(() => initialFps));
  let paused = $state(false);
  let frames = $state(0);
  let pass = $state(0);
  let cols = $state(1);
  let rows = $state(1);
  let error = $state("");
  let shown: FrameBatch | undefined;
  /** The colors the codes being shown, and the ones waiting, were made for. */
  let shownChannels: number[] = [];
  let readyChannels: number[] = [];
  let perPass = $state(untrack(() => info.framesPerPass));

  // Two-way transfer (a preview feature): the stream asks the receiver for
  // feedback, and a camera on this device watches the receiver's screen for
  // its feedback codes. The engine then sends only what is missing, and the
  // transfer ends by itself. Without the feature nothing here is active.
  const FEEDBACK_PREFIX = "QSF1-";
  // The feedback can drop out at any time (a hand in the way, the receiver
  // moved, its page closed). Sending never depends on it:
  /** after this long without it, stop waiting for answers (but trust the last one); */
  const FEEDBACK_QUIET_MS = 2000;
  /** after this long, assume nothing about the receiver and send everything again. */
  const FEEDBACK_LOST_MS = 10000;
  // Local network boost (a preview feature): once the two devices have seen
  // each other's screens they connect directly, and codes travel there too.
  // The screen keeps going: it sends the transfer from its other end, so both
  // channels bring something, and it is all there is if the connection drops.
  const lanFeature = featureOn("lan") && canConnect;
  let lan: LanSender | undefined;
  let link = $state<LinkState | undefined>();
  const linkUp = $derived(link === "connected");
  let linkTimer: ReturnType<typeof setTimeout> | undefined;

  // Remember trusted devices (a preview feature, on top of the local
  // network boost): a trusted device that has connected once is offered a
  // connection that needs no answer code from then on.
  const rememberFeature = lanFeature && featureOn("knownDevices");
  /** Name of the trusted device this connection is with, once it has said (and proven) who it is. */
  let remembered = $state("");

  /** The device to offer an answerless connection: the one this transfer is for, or the one seen last. */
  async function knownDevice(): Promise<Uint8Array | undefined> {
    if (!rememberFeature) return undefined;
    const linked = (await trustedDevices()).filter((d) => d.link);
    const fitting = info.recipients.length ? linked.filter((d) => info.recipients.includes(d.id)) : linked;
    const pick = fitting.sort((a, b) => b.link!.seen - a.link!.seen)[0];
    const bytes = pick?.link!.certificate.match(/../g)?.map((b) => parseInt(b, 16));
    return bytes?.length === 32 ? new Uint8Array(bytes) : undefined;
  }

  /**
   * The receiver says which device it is. Believed only if it is signed by that device's key, names the
   * certificate this very connection was made with, and the device is one this one trusts.
   */
  async function introduced(introduction: string, certificate: Uint8Array) {
    try {
      const said = JSON.parse(introduction) as { id: string; certificate: string; signature: string };
      const used = fingerprintHex(certificate);
      if (said.certificate !== used) return;
      await ready();
      const signature = Uint8Array.from(atob(said.signature), (c) => c.charCodeAt(0));
      if (!verifyDeviceSignature(said.id, introductionMessage(used), signature)) return;
      const device = parseDeviceId(said.id) as { fingerprint: string };
      remembered = (await rememberLink(device.fingerprint, used)) ?? "";
      log("tx", remembered ? "receiver is a trusted device: remembered" : "receiver introduced itself but is not trusted");
    } catch (e) {
      console.warn("introduction:", e);
    }
  }

  async function startLink() {
    const known = await knownDevice().catch(() => undefined);
    lan ??= new LanSender(
      info.session,
      {
      offer: (payload, id) => void engine.sendLinkOffer(payload, id).catch(() => {}),
      offerKnown: (payload, id) => void engine.sendLinkOffer(payload, id, true).catch(() => {}),
      offerSeeded: (payload, id) => void engine.sendLinkOffer(payload, id, KIND_SEEDED_OFFER).catch(() => {}),
      introduced: (introduction, certificate) => void introduced(introduction, certificate),
      pull: (count, binary, more) => engine.sendLink(count, binary, more),
      feedback: (code, taken) => hear(code, taken),
      state: (state) => {
        link = state;
        if (state === "offering") markLink("offered");
        if (state === "connected") markLink("connected");
        engine.sendTextChannelUp(state === "connected").catch(() => {});
        // A lost connection: the screen takes over again, and a new offer goes out.
        if (state === "closed" && !finished) linkTimer = setTimeout(() => lan?.start().catch(() => {}), 2000);
      },
      },
      { remembers: rememberFeature, known },
    );
    lan.start().catch(() => (link = "closed"));
  }

  // Feedback reaches this device through its camera (the receiver's screen),
  // through its microphone (the receiver's speaker: "Feedback by sound"), or
  // both; each is a preview feature of its own.
  const cameraFeature = featureOn("twoWay") || lanFeature;
  const soundFeature = featureOn("sound") && canListen;
  const twoWayFeature = cameraFeature || soundFeature;
  // Measurements (a preview feature): what this side spends its time on.
  const showStats = featureOn("stats");
  let eyeStats = $state<ScanStats | undefined>();
  /** The same, kept for the diagnostic log whether shown or not. */
  let eyeLatest: ScanStats | undefined;
  // When each step toward a network connection happened (seconds since sending began).
  type LinkStep = "offered" | "answer" | "connected";
  const began = performance.now();
  let linkSteps = $state<Partial<Record<LinkStep, number>>>({});
  function markLink(step: LinkStep) {
    if (linkSteps[step] !== undefined) return;
    linkSteps[step] = (performance.now() - began) / 1000;
    log("tx", `connection: ${step}`, { sinceStart: linkSteps[step] });
  }
  const afterLink = (at: number | undefined) => (at === undefined ? "not yet" : `after ${at.toFixed(1)} s`);
  let linkStats = $state<LanSender["measurements"] | undefined>();
  const percent = (share: number) => `${Math.round(share * 100)}%`;
  let ear: Ear | undefined;
  let watching = $state(false);
  let hearing = $state(false);
  let twoWay = $state(false);
  let eye: HTMLVideoElement;
  let scanner: Scanner | undefined;
  let report = $state<ReceiverReport | undefined>();
  // Automatic speed (a preview feature): the engine picks pictures per second
  // and the layout from what the receiver reports reading. The layouts on
  // offer are those that fit this screen, from one code to as many as stay
  // legible; the level is an index into them.
  const autoFeature = featureOn("autoTune") && featureOn("twoWay");
  let layouts: [number, number][] = [];
  /** Device pixels per dot of the codes in each layout. */
  let dots: number[] = [];
  let level = $state(0);
  let readShare = $state<number | undefined>();

  function offerLayouts() {
    if (!autoFeature || !canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const [w, h] = [canvas.clientWidth * dpr, canvas.clientHeight * dpr];
    const found: [number, number][] = [];
    for (let px = Math.floor(Math.min(w, h) / info.modules); px >= MIN_MODULE_PX; px--) {
      const [c, r] = fitGrid(info.modules, info.quiet, w, h, px);
      const last = found[found.length - 1];
      if (!last || c * r > last[0] * last[1]) found.push([c, r]);
    }
    if (found.length === 0) found.push([1, 1]);
    // Start from what was asked for (`grid` 0: as many as fit).
    const wanted = grid > 0 ? grid * grid : Infinity;
    const count = layouts.length ? layouts[level][0] * layouts[level][1] : wanted;
    layouts = found;
    const { modules, quiet } = info;
    dots = found.map(([c, r]) => Math.min(w / (c * (modules + quiet) + quiet), h / (r * (modules + quiet) + quiet)));
    level = Math.max(0, found.filter(([c, r]) => c * r <= count).length - 1);
    tune();
  }

  function tune() {
    if (!autoFeature || layouts.length === 0) return;
    engine.sendTune(layouts.map(([c, r]) => c * r * layers), dots, fps, level).catch(() => {});
  }

  let heardAt = 0;
  let quiet = $state(false);
  let silentFor = $state(0);
  let listenError = $state("");
  const finished = $derived(!!report?.complete);

  function onTexts(texts: string[]) {
    for (const t of texts) {
      if (!t.startsWith(LINK_PREFIX)) continue;
      markLink("answer");
      lan?.answer(t).catch((e) => console.warn("link answer:", e));
    }
    const text = texts.find((t) => t.startsWith(FEEDBACK_PREFIX));
    if (text) hear(text);
  }

  /** A feedback code from the receiver, by whatever way it came (`linkTaken`: through the connection). */
  function hear(text: string, linkTaken?: number) {
    if (finished) return;
    engine
      .sendFeedback(text, linkTaken)
      .then((r) => {
        if (!r) return;
        if (!report) log("tx", "first feedback", { by: linkTaken !== undefined ? "connection" : "camera or sound" });
        if (r.complete && !report?.complete) log("tx", "receiver has everything", { sinceStart: (performance.now() - began) / 1000 });
        report = r;
        if (r.fps != null && r.level != null) {
          fps = Math.round(r.fps * 10) / 10;
          level = Math.min(r.level, layouts.length - 1);
          log("tx", "automatic speed", { fps, layout: layouts[level]?.join("×"), readShare: r.readShare ?? undefined });
        }
        readShare = r.readShare ?? undefined;
        if (r.frames > readSoFar) {
          readSoFar = r.frames;
          readingAt = performance.now();
        }
        colorsRead(r.colors);
        heardAt = performance.now();
        arriving = arrival.update(heardAt, r.remainingBytes);
        quiet = false;
      })
      .catch(() => {});
  }

  async function listen(on: boolean) {
    twoWay = on;
    listenError = "";
    if (!on) {
      scanner?.stop();
      ear?.stop();
      watching = hearing = false;
      // With a direct connection the receiver answers there, camera or not.
      engine.sendAskForFeedback(linkUp, false).catch(() => {});
      if (!linkUp) forget();
      return;
    }
    const problems: string[] = [];
    const reason = (e: unknown, what: string) =>
      problems.push(e instanceof DOMException && e.name === "NotAllowedError" ? `${what} access denied` : `no ${what}`);
    if (cameraFeature) {
      try {
        scanner ??= new Scanner(eye, onTexts, (s) => {
          eyeLatest = s;
          if (showStats) eyeStats = s;
        });
        await scanner.start(undefined, "user");
        watching = true;
      } catch (e) {
        console.warn("two-way camera:", e);
        reason(e, "camera");
      }
    }
    if (soundFeature) {
      try {
        ear ??= new Ear((code) => hear(code));
        await ear.start();
        hearing = true;
      } catch (e) {
        console.warn("two-way microphone:", e);
        reason(e, "microphone");
      }
    }
    listenError = problems.join(", ");
    if (!watching && !hearing) twoWay = false;
    log("tx", "listening for feedback", { camera: watching, microphone: hearing, problems: listenError || undefined });
    engine.sendAskForFeedback(twoWay || linkUp, hearing).catch(() => {});
  }

  // Nothing more to learn once the receiver has everything.
  $effect(() => {
    if (!finished) return;
    scanner?.stop();
    ear?.stop();
    clearTimeout(linkTimer);
    lan?.stop();
  });

  function forget() {
    if (!report || finished) return;
    log("tx", "receiver not heard for long: sending everything again");
    report = undefined;
    quiet = false;
    engine.sendReceiverSilent(true).catch(() => {});
  }

  function watch() {
    const measured = linkUp ? lan?.measurements : undefined;
    if (showStats) linkStats = measured;
    logEvery("tx", 2000, "tx", "progress", () => ({
      codes: frames,
      pass: pass + 1,
      fps,
      layout: `${cols}×${rows}${layers > 1 ? `×${layers}` : ""}`,
      paused,
      receiverBytes: report ? report.totalBytes - report.remainingBytes : undefined,
      of: report?.totalBytes,
      arrivingPerS: report ? Math.round(arriving) : undefined,
      feedbackAgoS: report ? (performance.now() - heardAt) / 1000 : undefined,
      link,
      pace: measured ? measured.rate * 4096 : undefined,
      sent: measured?.sent,
      onTheirWay: measured?.onTheirWay,
      waitReceiver: measured?.waitingForReceiver,
      waitNetwork: measured?.waitingForNetwork,
      holdingBack: measured?.holdingBack,
      preparing: measured?.preparing,
      idle: measured?.nothingToSend,
      cameraReadsPerS: eyeLatest?.rate,
    }));
    colorsWatch();
    if (!report || finished) return;
    silentFor = performance.now() - heardAt;
    if (silentFor > FEEDBACK_LOST_MS) forget();
    else if (silentFor > FEEDBACK_QUIET_MS && !quiet) {
      quiet = true;
      log("tx", "receiver not heard for a moment: no longer waiting for it");
      engine.sendReceiverSilent(false).catch(() => {});
    }
  }

  function layout(): [number, number] {
    if (layouts.length) return layouts[level];
    if (grid > 0) return [grid, grid];
    const dpr = window.devicePixelRatio || 1;
    return fitGrid(info.modules, info.quiet, canvas.clientWidth * dpr, canvas.clientHeight * dpr, MIN_MODULE_PX);
  }

  // Color codes (a preview feature): several codes per cell, one in each of
  // the colors in use (0 red, 1 green, 2 blue). None: plain black and white.
  // `colors`: what was decided on. `channels`: what is being shown, which is
  // plain codes for as long as a receiver that should be heard is not.
  let colors = $state<number[]>(featureOn("color") ? [0, 1, 2] : []);
  let colorsHeld = $state(false);
  const channels = $derived(colorsHeld ? [] : colors);
  const layers = $derived(Math.max(1, channels.length));
  // The receiver is told which colors are in use: it cannot always tell by
  // looking (to a camera that does not keep them apart, color codes look
  // black and white).
  $effect(() => {
    const mask = channels.reduce((m, c) => m | (1 << c), 0);
    engine.sendColors(mask).catch(() => {});
  });
  // Colors are worth showing only as far as the receiver's camera tells them
  // apart. Its feedback says what it makes of each (which it reads, which
  // look alike to it: see `scan.worker.ts`); when that keeps saying the same
  // thing, the colors it gains nothing by go: one it does not read, all but
  // one of several it takes for the same. With fewer than two left, plain
  // codes are read better.
  const COLOR_NAMES = ["red", "green", "blue"];
  const COLOR_REPORTS = 4;
  /** After a change, what the receiver says is about the old colors for a while. */
  const COLOR_SETTLE_MS = 3000;
  let colorsProposed = 0;
  let colorsAgreed = 0;
  let colorsChangedAt = 0;
  let colorsNote = $state("");
  // Feedback says which colors a camera tells apart only if the camera reads
  // something. One that reads nothing while colors are shown (three codes on
  // top of each other, to a camera that sees them as one) never gets as far
  // as showing its feedback. So where feedback is expected and none is
  // heard, plain codes are shown until it is: a receiver that reads those
  // answers, and then the colors get their turn. If the receiver then stops
  // reading while it goes on being heard, the colors are what stopped it.
  const COLOR_SILENCE_MS = 8000;
  const COLOR_STUCK_MS = 6000;
  /** When the receiver last reported more codes read than before, and how many. */
  let readingAt = 0;
  let readSoFar = -1;
  function colorsWatch() {
    if (colors.length < 2 || finished || linkUp) return;
    const now = performance.now();
    if (!(watching || hearing)) {
      // Nobody is expected to answer: the colors stay as chosen.
      if (colorsHeld) colorsHeld = false;
      return;
    }
    const heardAgo = report ? now - heardAt : now - began;
    if (!colorsHeld) {
      if (heardAgo > COLOR_SILENCE_MS) {
        colorsHeld = true;
        colorsNote = "plain codes until the receiver is heard";
        log("tx", "no feedback while colors are shown: plain codes until there is");
        tune();
      } else if (report && !report.complete && heardAgo < 2000 && now - Math.max(readingAt, colorsChangedAt) > COLOR_STUCK_MS) {
        colors = [];
        colorsNote = "colors off: the receiver reads nothing while they are shown";
        log("tx", "the receiver is heard but reads nothing while colors are shown: plain codes from here on");
        tune();
      }
    } else if (report && heardAgo < 1500) {
      colorsHeld = false;
      colorsNote = "";
      colorsChangedAt = readingAt = now;
      log("tx", "the receiver is heard: colors again");
      tune();
    }
  }

  function colorsRead(seen: number | undefined) {
    if (channels.length < 2 || !seen || performance.now() - colorsChangedAt < COLOR_SETTLE_MS) return;
    const shownNow = channels.reduce((mask, c) => mask | (1 << c), 0);
    let keep: number;
    try {
      keep = colorsWorthShowing(shownNow, seen);
    } catch {
      return;
    }
    colorsAgreed = keep === colorsProposed ? colorsAgreed + 1 : 1;
    colorsProposed = keep;
    if (keep === shownNow || colorsAgreed < COLOR_REPORTS) return;
    const kept = [0, 1, 2].filter((c) => keep & (1 << c));
    colors = kept.length >= 2 ? kept : [];
    colorsNote =
      kept.length >= 2
        ? `colors: ${kept.map((c) => COLOR_NAMES[c]).join(" and ")} only (all the receiver's camera tells apart)`
        : "colors off: the receiver's camera does not tell them apart";
    colorsAgreed = 0;
    colorsChangedAt = performance.now();
    log("tx", "colors reduced to what the receiver tells apart", { seen: seen.toString(2), now: kept.map((c) => COLOR_NAMES[c]).join("+") || "plain" });
    tune();
  }

  /** White space beside the codes (CSS pixels on each side): room for the camera picture while aiming. */
  let sideRoom = $state(0);

  function draw() {
    if (!shown) return;
    // (In the colors the codes were made for, which may have changed since.)
    const scale =
      shownChannels.length > 0
        ? drawColorGrid(canvas, shown.data, info.modules, shown.count, cols, rows, info.quiet, shownChannels)
        : drawGrid(canvas, shown.data, info.modules, shown.count, cols, rows, info.quiet);
    const dpr = window.devicePixelRatio || 1;
    const used = (cols * (info.modules + info.quiet) + info.quiet) * scale;
    sideRoom = Math.max(0, (canvas.width - used) / 2 / dpr);
  }

  // Until the receiver has been seen (its answer or feedback code read),
  // whoever holds it up needs to see what this camera sees: the picture is
  // large then, beside the codes where there is room, and small afterwards.
  const aiming = $derived(watching && !report && !linkUp);

  onMount(() => {
    let raf = 0;
    let next = performance.now();
    let stopped = false;
    let ready: FrameBatch | undefined;
    let fetching = false;
    let lock: WakeLockSentinel | undefined;
    navigator.wakeLock?.request("screen").then((l) => (lock = l)).catch(() => {});
    root.requestFullscreen?.().catch(() => {});

    const fetchNext = () => {
      if (fetching || stopped) return;
      fetching = true;
      const [c, r] = layout();
      const colors = [...channels];
      engine
        .sendFrames(c * r * Math.max(1, colors.length))
        .then((batch) => {
          ready = batch;
          readyChannels = colors;
          [cols, rows] = [c, r];
        })
        .catch((e) => (error = e instanceof Error ? e.message : String(e)))
        .finally(() => (fetching = false));
    };

    const tick = (now: number) => {
      if (!paused && !finished && now >= next && ready) {
        shown = ready;
        shownChannels = readyChannels;
        ready = undefined;
        draw();
        frames = shown.frames;
        pass = shown.pass;
        perPass = shown.framesPerPass;
        next = Math.max(next + 1000 / fps, now);
      }
      if (!ready && !paused && !finished) fetchNext();
      raf = requestAnimationFrame(tick);
    };
    log("tx", "player", {
      qrDots: info.modules,
      bytesPerCode: info.symbolSize,
      wireBytes: info.wireBytes,
      codesPerPass: info.framesPerPass,
      fps,
      grid: grid || "fill",
      layers,
      encrypted: info.encrypted,
      canvas: `${Math.round(canvas.clientWidth * (window.devicePixelRatio || 1))}×${Math.round(canvas.clientHeight * (window.devicePixelRatio || 1))}`,
      twoWay: twoWayFeature,
      lan: lanFeature,
      remember: rememberFeature,
      auto: autoFeature,
    });
    fetchNext();
    raf = requestAnimationFrame(tick);
    if (twoWayFeature) listen(true);
    if (lanFeature) startLink();
    const watchdog = setInterval(watch, 500);
    offerLayouts();
    const onResize = () => {
      offerLayouts();
      draw();
    };
    window.addEventListener("resize", onResize);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === " ") paused = !paused;
      if (e.key === "+" || e.key === "ArrowUp") faster();
      if (e.key === "-" || e.key === "ArrowDown") slower();
      if (e.key === "Escape") stop();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      stopped = true;
      cancelAnimationFrame(raf);
      clearInterval(watchdog);
      clearTimeout(linkTimer);
      lan?.stop();
      ear?.stop();
      scanner?.dispose();
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", onKey);
      lock?.release().catch(() => {});
      engine.sendStop().catch(() => {});
    };
  });

  function faster() {
    fps = Math.min(30, Math.round(fps * 1.25 * 10) / 10);
    tune();
  }
  function slower() {
    fps = Math.max(1, Math.round((fps / 1.25) * 10) / 10);
    tune();
  }
  function stop() {
    log("tx", "player closed", { finished, codes: frames });
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    onclose();
  }

  const perTick = $derived(cols * rows * layers);
  // Codes still to show before this pass is over (a receiver that saw
  // everything is done by then).
  const inPass = $derived(frames % info.framesPerPass);
  const left = $derived(info.framesPerPass - inPass);
  const received = $derived(report ? report.totalBytes - report.remainingBytes : 0);
  // How fast the receiver's remainder shrinks, whatever channels bring it.
  const arrival = new RateMeter();
  let arriving = $state(0);
  const rate = $derived(info.symbolSize * fps * perTick);
</script>

<div class="player" bind:this={root}>
  <div class="stage">
    <canvas bind:this={canvas} data-frames={frames} aria-label="QR code stream"></canvas>
    {#if finished}
      <div class="finished" role="status">
        <p class="check">✓</p>
        <p><strong>Received.</strong> The other device has everything.</p>
        <button class="primary" onclick={stop}>Done</button>
      </div>
    {/if}
  </div>
  <div class="bar">
    <div class="info small">
      <strong>
        {info.summary}
        {#if linkUp}
          <span class="channel" data-testid="link-up">+ local network{remembered ? ` · ${remembered}` : ""}</span>
        {/if}
      </strong>
      <span>
        {#if error}
          {error}
        {:else if report}
          <span data-testid="receiver-report">
            Receiver: {Math.floor((received / Math.max(report.totalBytes, 1)) * 100)}% · {bytes(received)} of {bytes(report.totalBytes)},
            {bytes(report.remainingBytes)} to go{arriving > 0 ? ` · ${bytes(Math.round(arriving))}/s` : ""}
          </span>
          {#if quiet}
            <span data-testid="receiver-quiet">· not heard for {Math.round(silentFor / 1000)}s, sending on</span>
          {/if}
          · screen {cols}×{rows}{layers > 1 ? ` ×${layers} colors` : ""} ~{bytes(rate)}/s{autoFeature && readShare != null && !linkUp
            ? `, ${Math.round(readShare * 100)}% read`
            : ""}
          {#if colorsNote}<span data-testid="colors-reduced">· {colorsNote}</span>{/if}
        {:else}
          {#if info.resumed}<span data-testid="resumed">Only what is missing ({info.resumed.parts} of {info.resumed.of} parts) ·</span>{/if}
          Pass {pass + 1} · {inPass} of {info.framesPerPass} codes, {left} left ({duration(left / (fps * perTick))}) · {cols}×{rows}{layers > 1 ? ` ×${layers} colors` : ""} ·
          ~{bytes(rate)}/s
          {info.encrypted ? "· encrypted" : "· not encrypted"}
        {/if}
      </span>
    </div>
    {#if showStats && (linkStats || eyeStats || linkSteps.offered !== undefined)}
      <div class="info small" data-testid="tx-stats">
        {#if linkSteps.offered !== undefined}
          <span data-testid="tx-steps">
            Connection: offered {afterLink(linkSteps.offered)} · answer read {afterLink(linkSteps.answer)} · connected
            {afterLink(linkSteps.connected)}
          </span>
        {/if}
        {#if linkStats}
          <span>
            Network: {linkStats.sent} pieces sent{linkStats.binary ? "" : " as text"}, {linkStats.onTheirWay} on their way · pace
            {bytes(linkStats.rate * 4096)}/s · waiting for the receiver {percent(linkStats.waitingForReceiver)}, for the network
            {percent(linkStats.waitingForNetwork)}, holding back {percent(linkStats.holdingBack)}, preparing
            {percent(linkStats.preparing)}, nothing to send {percent(linkStats.nothingToSend)}
          </span>
        {/if}
        {#if eyeStats}
          <span>
            Camera: {eyeStats.rate.toFixed(0)} reads/s · {eyeStats.msWithCodes.toFixed(0)} ms with codes, {eyeStats.msWithout.toFixed(
              0,
            )} ms without
          </span>
        {/if}
      </div>
    {/if}
    <div class="row">
      <!-- svelte-ignore a11y_media_has_caption -->
      <video
        class="eye"
        class:on={watching}
        class:aiming
        class:beside={aiming && sideRoom >= 260}
        bind:this={eye}
        playsinline
        muted
        title="What this device's camera sees: hold the receiver's code into it"
      ></video>
      {#if twoWayFeature}
        <button class:active={twoWay} aria-pressed={twoWay} onclick={() => listen(!twoWay)} title="Take feedback from the receiver (its screen through this camera, its speaker through this microphone)">
          Two-way{hearing ? " 🎤" : ""}{listenError ? ` (${listenError})` : twoWay && !report ? " …" : ""}{link === "offering" ? " · offering LAN" : ""}
        </button>
      {/if}
      <button onclick={slower} aria-label="Slower">−</button>
      <span class="fps" data-testid="fps" title={autoFeature ? "Chosen from the receiver's feedback" : undefined}>
        {fps} fps{autoFeature && report && !linkUp ? " · auto" : ""}
      </span>
      <button onclick={faster} aria-label="Faster">+</button>
      <button onclick={() => (paused = !paused)}>{paused ? "Resume" : "Pause"}</button>
      <button class="primary" onclick={stop}>Done</button>
    </div>
  </div>
</div>

<style>
  .player {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: #fff;
    display: flex;
    flex-direction: column;
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
  }
  .bar {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    justify-content: space-between;
    padding: 8px 12px;
    padding-bottom: max(8px, env(safe-area-inset-bottom));
    background: #0f172a;
    color: #e2e8f0;
  }
  .bar button {
    background: #1e293b;
    color: #e2e8f0;
    border-color: #334155;
    min-height: 40px;
    padding: 6px 12px;
  }
  .bar button.primary {
    background: #38bdf8;
    color: #0b1120;
    border-color: #38bdf8;
  }
  .info {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .channel {
    margin-left: 6px;
    padding: 1px 8px;
    border-radius: 999px;
    background: #16a34a;
    color: #fff;
    font-weight: 600;
    font-size: 0.8rem;
  }
  .eye {
    display: none;
    height: 40px;
    border-radius: 6px;
    background: #000;
  }
  .eye.on {
    display: block;
  }
  .eye.aiming {
    height: 84px;
  }
  .eye.aiming.beside {
    position: fixed;
    top: 12px;
    left: 12px;
    height: auto;
    width: 240px;
    max-height: 40vh;
    z-index: 101;
    box-shadow: 0 2px 12px rgba(0, 0, 0, 0.35);
  }
  .bar button.active {
    border-color: #38bdf8;
    color: #38bdf8;
  }
  .finished {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    background: rgba(255, 255, 255, 0.96);
    color: #0f172a;
    text-align: center;
    padding: 16px;
  }
  .finished .check {
    font-size: 4rem;
    line-height: 1;
    color: #16a34a;
    margin: 0;
  }
  .fps {
    min-width: 4.5em;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
</style>
