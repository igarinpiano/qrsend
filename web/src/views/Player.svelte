<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { engine } from "../lib/engine";
  import type { FrameBatch, ReceiverReport, SendStarted } from "../lib/engine-types";
  import { drawColorGrid, drawGrid, fitGrid } from "../lib/qrdraw";
  import { bytes, duration, RateMeter } from "../lib/format";
  import { LINK_PREFIX, LanSender, canConnect, type LinkState } from "../lib/lan";
  import { featureOn } from "../lib/prefs";
  import { Scanner } from "../lib/scanner";

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

  function startLink() {
    lan ??= new LanSender(info.session, {
      offer: (payload, id) => void engine.sendLinkOffer(payload, id).catch(() => {}),
      pull: (count) => engine.sendTexts(count),
      feedback: (code) => hear(code),
      state: (state) => {
        link = state;
        engine.sendTextChannelUp(state === "connected").catch(() => {});
        // A lost connection: the screen takes over again, and a new offer goes out.
        if (state === "closed" && !finished) linkTimer = setTimeout(() => lan?.start().catch(() => {}), 2000);
      },
    });
    lan.start().catch(() => (link = "closed"));
  }

  const twoWayFeature = featureOn("twoWay") || lanFeature;
  let twoWay = $state(false);
  let eye: HTMLVideoElement;
  let scanner: Scanner | undefined;
  let report = $state<ReceiverReport | undefined>();
  let heardAt = 0;
  let quiet = $state(false);
  let silentFor = $state(0);
  let listenError = $state("");
  const finished = $derived(!!report?.complete);

  function onTexts(texts: string[]) {
    for (const t of texts) if (t.startsWith(LINK_PREFIX)) lan?.answer(t).catch((e) => console.warn("link answer:", e));
    const text = texts.find((t) => t.startsWith(FEEDBACK_PREFIX));
    if (text) hear(text);
  }

  /** A feedback code from the receiver, by whatever way it came. */
  function hear(text: string) {
    if (finished) return;
    engine
      .sendFeedback(text)
      .then((r) => {
        if (!r) return;
        report = r;
        heardAt = performance.now();
        arriving = arrival.update(heardAt, r.remainingBytes);
        quiet = false;
      })
      .catch(() => {});
  }

  async function listen(on: boolean) {
    twoWay = on;
    listenError = "";
    // With a direct connection the receiver answers there, camera or not.
    engine.sendAskForFeedback(on || linkUp).catch(() => {});
    if (!on) {
      scanner?.stop();
      if (!linkUp) forget();
      return;
    }
    try {
      scanner ??= new Scanner(eye, onTexts);
      await scanner.start(undefined, "user");
    } catch (e) {
      console.warn("two-way camera:", e);
      listenError = e instanceof DOMException && e.name === "NotAllowedError" ? "camera access denied" : "no camera";
      twoWay = false;
      engine.sendAskForFeedback(false).catch(() => {});
    }
  }

  // Nothing more to learn once the receiver has everything.
  $effect(() => {
    if (!finished) return;
    scanner?.stop();
    clearTimeout(linkTimer);
    lan?.stop();
  });

  function forget() {
    if (!report || finished) return;
    report = undefined;
    quiet = false;
    engine.sendReceiverSilent(true).catch(() => {});
  }

  function watch() {
    if (!report || finished) return;
    silentFor = performance.now() - heardAt;
    if (silentFor > FEEDBACK_LOST_MS) forget();
    else if (silentFor > FEEDBACK_QUIET_MS && !quiet) {
      quiet = true;
      engine.sendReceiverSilent(false).catch(() => {});
    }
  }

  function layout(): [number, number] {
    if (grid > 0) return [grid, grid];
    const dpr = window.devicePixelRatio || 1;
    return fitGrid(info.modules, info.quiet, canvas.clientWidth * dpr, canvas.clientHeight * dpr, MIN_MODULE_PX);
  }

  // Color codes (a preview feature): three codes per cell.
  const layers = featureOn("color") ? 3 : 1;

  function draw() {
    if (!shown) return;
    const paint = layers === 3 ? drawColorGrid : drawGrid;
    paint(canvas, shown.data, info.modules, shown.count, cols, rows, info.quiet);
  }

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
      engine
        .sendFrames(c * r * layers)
        .then((batch) => {
          ready = batch;
          [cols, rows] = [c, r];
        })
        .catch((e) => (error = e instanceof Error ? e.message : String(e)))
        .finally(() => (fetching = false));
    };

    const tick = (now: number) => {
      if (!paused && !finished && now >= next && ready) {
        shown = ready;
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
    fetchNext();
    raf = requestAnimationFrame(tick);
    if (twoWayFeature) listen(true);
    if (lanFeature) startLink();
    const watchdog = setInterval(watch, 500);
    const onResize = () => draw();
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
      scanner?.dispose();
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", onKey);
      lock?.release().catch(() => {});
      engine.sendStop().catch(() => {});
    };
  });

  function faster() {
    fps = Math.min(30, Math.round(fps * 1.25 * 10) / 10);
  }
  function slower() {
    fps = Math.max(1, Math.round((fps / 1.25) * 10) / 10);
  }
  function stop() {
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
        {#if linkUp}<span class="channel" data-testid="link-up">+ local network</span>{/if}
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
          · screen {cols}×{rows}{layers === 3 ? " ×3 colors" : ""} ~{bytes(rate)}/s
        {:else}
          Pass {pass + 1} · {inPass} of {info.framesPerPass} codes, {left} left ({duration(left / (fps * perTick))}) · {cols}×{rows}{layers === 3 ? " ×3 colors" : ""} ·
          ~{bytes(rate)}/s
          {info.encrypted ? "· encrypted" : "· not encrypted"}
        {/if}
      </span>
    </div>
    <div class="row">
      <!-- svelte-ignore a11y_media_has_caption -->
      <video class="eye" class:on={twoWay} bind:this={eye} playsinline muted></video>
      {#if twoWayFeature}
        <button class:active={twoWay} aria-pressed={twoWay} onclick={() => listen(!twoWay)} title="Watch the receiver's screen for feedback codes">
          Two-way{listenError ? ` (${listenError})` : twoWay && !report ? " …" : ""}{link === "offering" ? " · offering LAN" : ""}
        </button>
      {/if}
      <button onclick={slower} aria-label="Slower">−</button>
      <span class="fps">{fps} fps</span>
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
