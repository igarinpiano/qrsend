<script lang="ts">
  import { onMount } from "svelte";
  import { Scanner, type ScanStats } from "../lib/scanner";

  let {
    ontexts,
    active = true,
    allowFile = false,
  }: { ontexts: (texts: string[]) => void; active?: boolean; allowFile?: boolean } = $props();

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
    try {
      await scanner!.start(cameraId || undefined);
      started = true;
      cameras = await Scanner.cameras();
    } catch (e) {
      started = false;
      error =
        e instanceof DOMException && e.name === "NotAllowedError"
          ? "Camera access was denied. Allow it in the browser settings and try again."
          : `Cannot open the camera: ${e instanceof Error ? e.message : e}`;
    }
  }

  async function scanFile(file: File | undefined) {
    if (!file || !scanner) return;
    error = "";
    started = true;
    fileProgress = { name: file.name, at: 0, duration: 0, ended: false };
    try {
      await scanner.scanFile(file, (at, duration) => {
        if (fileProgress) fileProgress = { ...fileProgress, at, duration };
      });
      if (fileProgress) fileProgress = { ...fileProgress, ended: true };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      fileProgress = undefined;
    }
    started = false;
  }

  let ready = $state(false);
  let autoStarted = false;

  onMount(() => {
    scanner = new Scanner(video, (t) => ontexts(t), (s) => (stats = s));
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

<div class="camera">
  <!-- svelte-ignore a11y_media_has_caption -->
  <video bind:this={video} playsinline muted></video>
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
    {#if stats}{stats.width}×{stats.height} · {stats.engine === "native" ? "built-in detector" : "ZXing"} · {stats.codes} codes{/if}
  </span>
  <span class="row">
    {#if cameras.length > 1 && !fileProgress}
      <select class="picker" bind:value={cameraId} onchange={start} aria-label="Camera">
        <option value="">Default camera</option>
        {#each cameras as c (c.deviceId)}
          <option value={c.deviceId}>{c.label || "Camera"}</option>
        {/each}
      </select>
    {/if}
    {#if allowFile && active}
      <label class="button file">
        Use a video file
        <input type="file" accept="video/*" hidden data-testid="video-file" onchange={(e) => scanFile(e.currentTarget.files?.[0])} />
      </label>
    {/if}
  </span>
</div>

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
