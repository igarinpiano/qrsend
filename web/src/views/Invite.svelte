<script lang="ts">
  // For a receiver who has no QRSend open: an ordinary QR code with the
  // address of the Receive page, which the camera app of any phone opens,
  // the address itself to type (a computer seldom reads codes), and the
  // system's share sheet where there is one (AirDrop, Nearby Share,
  // messages).
  import { ready, renderText } from "../lib/core";
  import { receiveLink } from "../lib/invite";
  import { log } from "../lib/log";
  import { toDataUrl } from "../lib/qrdraw";
  import { copyText } from "../lib/save";

  let { big = false }: { /** As large as the room allows (in the player). */ big?: boolean } = $props();

  const link = receiveLink();
  let qr = $state("");
  let done = $state("");
  const canShare = typeof navigator.share === "function";

  $effect(() => {
    ready()
      .then(() => (qr = toDataUrl(renderText(link), 8)))
      .catch(() => (qr = ""));
  });

  async function share() {
    try {
      await navigator.share({ title: "QRSend", text: "Open QRSend to receive", url: link });
      log("invite", "shared");
    } catch {
      // Dismissed, or nothing to share with: nothing to say.
    }
  }
  async function copy() {
    try {
      await copyText(link);
      done = "Copied";
    } catch {
      done = "";
    }
  }
</script>

<div class="invite" class:big data-testid="invite">
  {#if qr}<img src={qr} alt="A QR code that opens QRSend to receive" data-testid="invite-code" />{/if}
  <div class="words stack">
    <p>
      <strong>No QRSend on the other device?</strong> Point its camera at this code: the phone's own camera app is
      enough. It opens QRSend in the browser, ready to receive. Nothing to install.
    </p>
    <p class="small">Or open this address there:</p>
    <p class="link mono" data-testid="invite-link">{link}</p>
    <div class="row">
      {#if canShare}<button onclick={share}>Share link</button>{/if}
      <button onclick={copy}>{done || "Copy link"}</button>
    </div>
  </div>
</div>

<style>
  .invite {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
    align-items: center;
  }
  img {
    width: 180px;
    max-width: 100%;
    image-rendering: pixelated;
    border-radius: 6px;
    background: #fff;
  }
  .words {
    flex: 1 1 220px;
    min-width: 0;
  }
  p {
    margin: 0;
  }
  .link {
    overflow-wrap: anywhere;
    user-select: all;
  }
  .big {
    flex-direction: column;
    justify-content: center;
    padding: 16px 16px 0;
    color: #0f172a;
    text-align: center;
  }
  .big img {
    width: min(60vmin, 100%, 480px);
    flex: none;
  }
  .big .words {
    flex: none;
    max-width: 560px;
    align-items: center;
  }
  .big .row {
    justify-content: center;
  }
</style>
