<script lang="ts">
  import { onMount } from "svelte";
  import { ADVICE } from "../lib/guide";
  import { log, logEvery } from "../lib/log";
  import { featureOn } from "../lib/prefs";
  import { Scanner, type ScanStats } from "../lib/scanner";

  let {
    ontexts,
    oncamera,
    active = true,
    allowFile = false,
    compact = false,
    advise = true,
    senderColors = -1,
  }: {
    ontexts: (texts: string[]) => void;
    /** What the camera makes of the codes: pictures read per second, camera pixels per dot. */
    oncamera?: (reads: number, dot: number, colors: number) => void;
    active?: boolean;
    allowFile?: boolean;
    /** Take little room: something else on the page matters more right now. */
    compact?: boolean;
    /** Whether advice on holding the camera is wanted (not while the camera has nothing it must see). */
    advise?: boolean;
    /** What the sender said about the colors of its codes (-1: nothing). */
    senderColors?: number;
  } = $props();

  let video: HTMLVideoElement;
  let scanner: Scanner | undefined;
  let error = $state("");
  let stats = $state<ScanStats | undefined>();
  let cameras = $state<MediaDeviceInfo[]>([]);
  let cameraId = $state("");
  let started = $state(false);
  let fileProgress = $state<{ name: string; at: number; duration: number; ended: boolean } | undefined>();

  async function start() {
    error = "";
    fileProgress = undefined;
    onScreen = false;
    try {
      await scanner!.start(cameraId || undefined);
      started = true;
      cameras = await Scanner.cameras();
      log("camera", "started", { cameras: cameras.length, chosen: cameraId ? "picked" : "default" });
    } catch (e) {
      started = false;
      log("camera", "failed", { error: e instanceof Error ? `${e.name}: ${e.message}`.slice(0, 160) : String(e) });
      error =
        e instanceof DOMException && e.name === "NotAllowedError"
          ? "Camera access was denied. Allow it in the browser settings and try again."
          : `Cannot open the camera: ${e instanceof Error ? e.message : e}`;
    }
  }

  const showStats = featureOn("stats");
  // Camera guidance (a preview feature): advice on how to hold the camera.
  // A captured screen or a video file needs none.
  const guideFeature = featureOn("guide");


  // Receiving from the screen (a preview feature, and only where the browser can capture one).
  const screenOffered = featureOn("screenCapture") && Scanner.canCaptureScreen;
  let onScreen = $state(false);
  const advice = $derived(guideFeature && advise && started && !onScreen && !fileProgress ? stats?.advice : undefined);

  async function useScreen() {
    if (!scanner) return;
    error = "";
    fileProgress = undefined;
    try {
      await scanner.startScreen(() => {
        started = false;
        onScreen = false;
      });
      started = true;
      onScreen = true;
    } catch (e) {
      // Closing the picker is not an error worth showing.
      if (!(e instanceof DOMException && e.name === "NotAllowedError")) {
        error = `Cannot capture the screen: ${e instanceof Error ? e.message : e}`;
      }
    }
  }

  async function scanFile(file: File | undefined) {
    if (!file || !scanner) return;
    error = "";
    started = true;
    onScreen = false;
    fileProgress = { name: file.name, at: 0, duration: 0, ended: false };
    try {
      const ended = await scanner.scanFile(file, (at, duration) => {
        if (fileProgress) fileProgress = { ...fileProgress, at, duration };
      });
      if (!ended) return; // the camera or another file took over
      if (fileProgress) fileProgress = { ...fileProgress, ended: true };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      fileProgress = undefined;
    }
    started = false;
  }

  let ready = $state(false);
  $effect(() => {
    const colors = senderColors;
    if (ready) scanner?.declareColors(colors);
  });
  let autoStarted = false;

  onMount(() => {
    scanner = new Scanner(
      video,
      (t) => ontexts(t),
      (s) => {
        stats = s;
        logEvery("camera", 2000, "camera", "reading", () => ({
          source: onScreen ? "screen" : fileProgress ? "file" : "camera",
          picture: `${s.width}×${s.height}`,
          decoder: s.engine,
          pictures: s.frames,
          readsPerS: s.rate,
          msWithCodes: s.msWithCodes,
          msWithout: s.msWithout,
          codes: s.codes,
          color: !!s.colored,
          colorsRead: s.colors || undefined,
          dotPx: s.dot,
          advice: s.advice,
        }));
        // Only a camera's numbers say something about the camera.
        if (!onScreen && !fileProgress) oncamera?.(s.rate, s.dot, s.colors ?? 0);
      },
    );
    scanner.guide(guideFeature);
    ready = true;
    return () => scanner?.dispose();
  });

  // Open the camera once the parent is ready for codes; release it when the
  // parent is done.
  $effect(() => {
    if (!ready) return;
    if (active && !autoStarted) {
      autoStarted = true;
      start();
    } else if (!active && started) {
      scanner!.stop();
      started = false;
    }
  });
</script>

<div class="camera" class:compact>
  <!-- svelte-ignore a11y_media_has_caption -->
  <video bind:this={video} playsinline muted></video>
  {#if advice}
    <p class="advice" role="status" data-testid="camera-advice">{ADVICE[advice]}</p>
  {/if}
  {#if !started}
    <div class="overlay">
      {#if error}<p>{error}</p>{/if}
      {#if fileProgress?.ended}<p>Reached the end of {fileProgress.name}.</p>{/if}
      {#if active}<button class="primary" onclick={start}>Start camera</button>{/if}
    </div>
  {/if}
</div>
<div class="row spread small muted">
  <span>
    {#if fileProgress && !fileProgress.ended}
      {fileProgress.name}: {fileProgress.at.toFixed(1)} / {fileProgress.duration.toFixed(1)} s ·
    {/if}
    {#if stats}{onScreen ? "screen " : ""}{stats.width}×{stats.height} · {stats.engine === "native" ? "built-in detector" : "ZXing"} ·
      {stats.codes} codes{stats.colored ? " · color" : ""}{/if}
    {#if stats && showStats}
      <span data-testid="scan-stats">
        · {stats.rate.toFixed(0)} reads/s · {stats.msWithCodes.toFixed(0)} ms with codes, {stats.msWithout.toFixed(0)} ms without
      </span>
    {/if}
  </span>
  <span class="row">
    {#if cameras.length > 1 && !fileProgress && !onScreen}
      <select class="picker" bind:value={cameraId} onchange={start} aria-label="Camera">
        <option value="">Default camera</option>
        {#each cameras as c (c.deviceId)}
          <option value={c.deviceId}>{c.label || "Camera"}</option>
        {/each}
      </select>
    {/if}
    {#if screenOffered && active}
      <button class="file" onclick={onScreen ? start : useScreen}>{onScreen ? "Use the camera" : "Use the screen"}</button>
    {/if}
    {#if allowFile && active}
      <label class="button file">
        Use a video file
        <input type="file" accept="video/*" hidden data-testid="video-file" onchange={(e) => scanFile(e.currentTarget.files?.[0])} />
      </label>
    {/if}
  </span>
</div>

{#if onScreen}
  <p class="small muted">
    A captured screen is read pixel for pixel: on the sender, choose “Fill the screen” for the codes and a higher speed —
    one code at a time is as slow here as with a camera.
  </p>
{/if}

<style>
  .camera {
    position: relative;
    background: #000;
    border-radius: var(--radius);
    overflow: hidden;
    aspect-ratio: 4 / 3;
    max-height: 60vh;
    margin-bottom: 8px;
  }
  .camera.compact {
    aspect-ratio: auto;
    height: 22vh;
    min-height: 96px;
  }
  video {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-content: center;
    gap: 12px;
    padding: 16px;
    text-align: center;
    color: #fff;
  }
  .advice {
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: 8px;
    margin: 0;
    padding: 8px 12px;
    border-radius: 8px;
    background: rgba(15, 23, 42, 0.85);
    color: #fff;
    text-align: center;
  }
  .picker {
    width: auto;
    max-width: 14em;
    padding: 4px 8px;
  }
  .file {
    min-height: 32px;
    padding: 4px 10px;
    font-size: 0.85rem;
  }
</style>
