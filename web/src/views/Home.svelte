<script lang="ts">
  import { onMount } from "svelte";
  import { listSessions } from "../lib/inbox";

  let pending = $state(0);
  onMount(async () => {
    pending = (await listSessions()).filter((s) => !s.complete).length;
  });
</script>

<section class="hero">
  <h1>Send anything through QR codes</h1>
  <p class="muted">
    Text, files and whole folders travel as a stream of QR codes from one screen to another camera.
    No network, no accounts — everything happens on your devices.
  </p>
</section>

<div class="actions">
  <a class="button primary big" href="#/send">Send</a>
  <a class="button big" href="#/receive">Receive</a>
</div>

{#if pending > 0}
  <p class="center"><a href="#/inbox">{pending} unfinished transfer{pending === 1 ? "" : "s"} in your inbox</a></p>
{/if}

<div class="card stack small">
  <h3>How it works</h3>
  <p>
    The sender shows an endless, fountain-coded stream of codes. The receiver only needs enough of them, in any order — missed
    frames don’t matter, and long transfers can be paused and resumed.
  </p>
  <p>
    For private transfers, pair your devices once on the <a href="#/devices">Devices</a> page: data is then end-to-end
    encrypted for the receiving device and signed by the sender.
  </p>
  <p class="muted">Works offline once loaded. Also available as a command-line tool: <code>qrsend</code>.</p>
</div>

<style>
  .hero {
    padding: 24px 0 8px;
  }
  .hero h1 {
    font-size: clamp(1.6rem, 5vw, 2.2rem);
  }
  .actions {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
    margin: 16px 0 20px;
  }
  .big {
    min-height: 96px;
    font-size: 1.3rem;
  }
  .center {
    text-align: center;
  }
</style>
