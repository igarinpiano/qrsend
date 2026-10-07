<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { engine } from "../lib/engine";
  import type { FrameBatch, SendStarted } from "../lib/engine-types";
  import { drawGrid, fitGrid } from "../lib/qrdraw";
  import { bytes, duration } from "../lib/format";

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

  function layout(): [number, number] {
    if (grid > 0) return [grid, grid];
    const dpr = window.devicePixelRatio || 1;
    return fitGrid(info.modules, info.quiet, canvas.clientWidth * dpr, canvas.clientHeight * dpr, MIN_MODULE_PX);
  }

  function draw() {
    if (shown) drawGrid(canvas, shown.data, info.modules, shown.count, cols, rows, info.quiet);
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
        .sendFrames(c * r)
        .then((batch) => {
          ready = batch;
          [cols, rows] = [c, r];
        })
        .catch((e) => (error = e instanceof Error ? e.message : String(e)))
        .finally(() => (fetching = false));
    };

    const tick = (now: number) => {
      if (!paused && now >= next && ready) {
        shown = ready;
        ready = undefined;
        draw();
        frames = shown.frames;
        pass = shown.pass;
        next = Math.max(next + 1000 / fps, now);
      }
      if (!ready && !paused) fetchNext();
      raf = requestAnimationFrame(tick);
    };
    fetchNext();
    raf = requestAnimationFrame(tick);
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

  const perTick = $derived(cols * rows);
  // Codes still to show before this pass is over (a receiver that saw
  // everything is done by then).
  const inPass = $derived(frames % info.framesPerPass);
  const left = $derived(info.framesPerPass - inPass);
  const rate = $derived(info.symbolSize * fps * perTick);
</script>

<div class="player" bind:this={root}>
  <canvas bind:this={canvas} data-frames={frames} aria-label="QR code stream"></canvas>
  <div class="bar">
    <div class="info small">
      <strong>{info.summary}</strong>
      <span>
        {#if error}
          {error}
        {:else}
          Pass {pass + 1} · {inPass} of {info.framesPerPass} codes, {left} left ({duration(left / (fps * perTick))}) · {cols}×{rows} ·
          ~{bytes(rate)}/s
          {info.encrypted ? "· encrypted" : "· not encrypted"}
        {/if}
      </span>
    </div>
    <div class="row">
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
  canvas {
    flex: 1;
    width: 100%;
    min-height: 0;
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
  .fps {
    min-width: 4.5em;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
</style>
