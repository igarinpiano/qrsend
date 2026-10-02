<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type { SendSession } from "../lib/core";
  import { drawGrid, type Matrix } from "../lib/qrdraw";
  import { bytes, duration } from "../lib/format";

  let { session, fps: initialFps, grid, onclose }: { session: SendSession; fps: number; grid: number; onclose: () => void } =
    $props();

  let canvas: HTMLCanvasElement;
  let root: HTMLDivElement;
  let fps = $state(untrack(() => initialFps));
  let paused = $state(false);
  let frames = $state(0);
  let pass = $state(0);
  let codes: Matrix[] = [];
  const perPass = $derived(session.framesPerPass);
  const symbol = $derived(session.symbolSize);

  onMount(() => {
    let raf = 0;
    let next = performance.now();
    let lock: WakeLockSentinel | undefined;
    navigator.wakeLock?.request("screen").then((l) => (lock = l)).catch(() => {});
    root.requestFullscreen?.().catch(() => {});

    const tick = (now: number) => {
      if (!paused && now >= next) {
        codes = Array.from({ length: grid * grid }, () => session.nextQr());
        drawGrid(canvas, codes, grid);
        frames = session.frames;
        pass = session.pass;
        next = Math.max(next + 1000 / fps, now);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    const onResize = () => drawGrid(canvas, codes, grid);
    window.addEventListener("resize", onResize);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === " ") paused = !paused;
      if (e.key === "+" || e.key === "ArrowUp") faster();
      if (e.key === "-" || e.key === "ArrowDown") slower();
      if (e.key === "Escape") stop();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", onKey);
      lock?.release().catch(() => {});
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

  const progress = $derived(((frames % perPass) / perPass) * 100);
  const rate = $derived(symbol * fps * grid * grid);
</script>

<div class="player" bind:this={root}>
  <canvas bind:this={canvas} data-frames={frames} aria-label="QR code stream"></canvas>
  <div class="bar">
    <div class="info small">
      <strong>{session.summary}</strong>
      <span>
        Pass {pass + 1} · {progress.toFixed(0)}% · {frames} codes · ~{bytes(rate)}/s · one pass ≈ {duration(perPass / (fps * grid * grid))}
        {session.encrypted ? "· encrypted" : "· not encrypted"}
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
