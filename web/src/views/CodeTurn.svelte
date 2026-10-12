<script lang="ts">
  // Several QR codes shown one after the other, round and round (what does not fit one code that reads well: a
  // device ID with post-quantum keys, a one-time key). Whoever reads them takes them in any order; the one who
  // shows them may hold a code, or step through them by hand.
  import { onDestroy } from "svelte";
  import { renderText } from "../lib/core";
  import { toDataUrl } from "../lib/qrdraw";

  let { codes, alt, testid }: { codes: string[]; alt: string; testid: string } = $props();

  const pictures = $derived(codes.map((code) => toDataUrl(renderText(code), 5)));
  let showing = $state(0);
  let playing = $state(true);
  const CODE_MS = 700;
  const turn = setInterval(() => {
    if (playing && pictures.length > 1) showing = (showing + 1) % pictures.length;
  }, CODE_MS);
  onDestroy(() => clearInterval(turn));
  function step(by: number) {
    playing = false;
    showing = (showing + by + pictures.length) % pictures.length;
  }
  const at = $derived(Math.min(showing, pictures.length - 1));
</script>

<div class="turn">
  <img src={pictures[at]} {alt} data-testid={testid} data-part={at + 1} data-parts={pictures.length} />
  {#if pictures.length > 1}
    <div class="row small">
      <button class="link" onclick={() => step(-1)} aria-label="Previous code">◀</button>
      <span>Code {at + 1} of {pictures.length}</span>
      <button class="link" onclick={() => step(1)} aria-label="Next code">▶</button>
      <button class="link" onclick={() => (playing = !playing)}>{playing ? "Hold" : "Go on"}</button>
    </div>
  {/if}
</div>

<style>
  .turn {
    width: min(300px, 100%);
  }
  img {
    width: 100%;
    image-rendering: pixelated;
    border-radius: 8px;
    background: #fff;
  }
  .link {
    min-height: 0;
    padding: 2px 6px;
    font-weight: 500;
    font-size: 0.85rem;
  }
</style>
