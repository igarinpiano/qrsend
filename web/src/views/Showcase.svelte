<script lang="ts">
  // A code this device shows to the other one's camera (the answer to an
  // offer to connect, feedback): as large as the screen allows, at the edge
  // where the other camera is most likely looking, and full screen on a tap.
  import { onMount, type Snippet } from "svelte";
  import { log } from "../lib/log";

  let {
    src,
    alt,
    testid,
    code,
    edge,
    onmove,
    children,
  }: {
    src: string;
    alt: string;
    testid: string;
    /** The code's text (for tests). */
    code?: string;
    /** The edge of the screen the code keeps to when full screen. */
    edge: "top" | "bottom" | "left" | "right";
    /** Asks for the code at the other end of the screen. */
    onmove: () => void;
    children: Snippet;
  } = $props();

  let full = $state(false);
  let box: HTMLDivElement;

  onMount(() => {
    // It appears while the eyes are on the other device: bring it into view.
    box.scrollIntoView({ block: "nearest", behavior: "smooth" });
    log("rx", "showing a code", { what: testid, edge });
  });

  function toggle() {
    full = !full;
    log("rx", full ? "code full screen" : "code back in the page", { what: testid, edge });
  }
</script>

<div class="showcase {edge}" class:full bind:this={box}>
  <button class="code" onclick={toggle} aria-label={full ? "Leave full screen" : "Show this code full screen"}>
    <img {src} {alt} data-testid={testid} data-code={code} />
  </button>
  <div class="about stack">
    {@render children()}
    <div class="row">
      <button onclick={toggle}>{full ? "Close" : "Full screen"}</button>
      <button onclick={onmove} title="Where the other device's camera sees it best">Move to the other end</button>
    </div>
  </div>
</div>

<style>
  .showcase {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }
  .code {
    padding: 0;
    border: 0;
    background: #fff;
    border-radius: 8px;
    line-height: 0;
    cursor: zoom-in;
    /* As large as fits, leaving room to see that there is more below. */
    width: min(100%, 62vh);
  }
  img {
    width: 100%;
    image-rendering: pixelated;
    border-radius: 8px;
  }
  .about {
    width: 100%;
  }
  .about :global(p) {
    margin: 0;
  }

  /* Full screen: white all around (a camera exposes for it), the code at the chosen edge. */
  .full {
    position: fixed;
    inset: 0;
    z-index: 90;
    border: 0;
    border-radius: 0;
    background: #fff;
    color: #0f172a;
    padding: max(12px, env(safe-area-inset-top)) max(12px, env(safe-area-inset-right)) max(12px, env(safe-area-inset-bottom))
      max(12px, env(safe-area-inset-left));
    justify-content: space-between;
  }
  .full .code {
    cursor: zoom-out;
    width: min(100vw - 24px, 100vh - 150px);
  }
  .full.bottom {
    flex-direction: column-reverse;
  }
  .full.left,
  .full.right {
    flex-direction: row;
    align-items: center;
  }
  .full.right {
    flex-direction: row-reverse;
  }
  .full.left .code,
  .full.right .code {
    width: min(100vh - 24px, 62vw);
    flex: none;
  }
  .full .about :global(.muted) {
    color: #475569;
  }
  .full .about button {
    background: #f1f5f9;
    color: #0f172a;
    border-color: #cbd5e1;
  }
</style>
