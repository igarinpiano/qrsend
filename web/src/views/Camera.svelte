<script lang="ts">
  import { onMount } from "svelte";
  import { Scanner, type ScanStats } from "../lib/scanner";

  let { ontexts, active = true }: { ontexts: (texts: string[]) => void; active?: boolean } = $props();

  let video: HTMLVideoElement;
  let scanner: Scanner | undefined;
  let error = $state("");
  let stats = $state<ScanStats | undefined>();
  let cameras = $state<MediaDeviceInfo[]>([]);
  let cameraId = $state("");
  let started = $state(false);

  async function start() {
    error = "";
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

  onMount(() => {
    scanner = new Scanner(video, (t) => ontexts(t), (s) => (stats = s));
    if (active) start();
    return () => scanner?.dispose();
  });

  $effect(() => {
    if (!scanner) return;
    if (!active && started) {
      scanner.stop();
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
      {#if active}<button class="primary" onclick={start}>Start camera</button>{/if}
    </div>
  {/if}
</div>
<div class="row spread small muted">
  <span>
    {#if stats}{stats.width}×{stats.height} · {stats.engine === "native" ? "built-in detector" : "ZXing"} · {stats.codes} codes{/if}
  </span>
  {#if cameras.length > 1}
    <select class="picker" bind:value={cameraId} onchange={start} aria-label="Camera">
      <option value="">Default camera</option>
      {#each cameras as c (c.deviceId)}
        <option value={c.deviceId}>{c.label || "Camera"}</option>
      {/each}
    </select>
  {/if}
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
    max-width: 60%;
    padding: 4px 8px;
  }
</style>
