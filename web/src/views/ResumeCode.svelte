<script lang="ts">
  // How an unfinished transfer is carried on, for whoever sits at the
  // sender: the resume code large enough to read off this screen and type
  // (the two devices often share no clipboard), as a QR code for a third
  // device, and to copy where that helps.
  import { ready, renderText } from "../lib/core";
  import { toDataUrl } from "../lib/qrdraw";
  import { copyText } from "../lib/save";

  let { code }: { code: string } = $props();

  const command = $derived(`qrsend send --resume ${code}`);
  let qr = $state("");
  let copied = $state("");
  $effect(() => {
    const text = command;
    ready()
      .then(() => (qr = toDataUrl(renderText(text), 4)))
      .catch(() => (qr = ""));
  });

  async function copy(what: string, text: string) {
    try {
      await copyText(text);
      copied = what;
    } catch {
      copied = "";
    }
  }
</script>

<div class="resume stack" data-testid="resume-code">
  <p class="small">
    <strong>From a browser:</strong> send the very same data again, unchanged. It is the same transfer, and this device
    carries on where it stopped. (Not if it was encrypted for a device: that has to be received from the start.)
  </p>
  <p class="small">
    <strong>From the command line:</strong> run this on the sending computer to send only what is missing.
  </p>
  <p class="code mono" data-testid="resume-text">qrsend send --resume <span class="value">{code}</span></p>
  <div class="row">
    {#if qr}<img src={qr} alt="The resume command as a QR code" />{/if}
    <div class="stack">
      <button onclick={() => copy("code", code)}>{copied === "code" ? "Copied" : "Copy code"}</button>
      <button onclick={() => copy("command", command)}>{copied === "command" ? "Copied" : "Copy command"}</button>
    </div>
  </div>
</div>

<style>
  .code {
    margin: 0;
    font-size: 1.05rem;
    overflow-wrap: anywhere;
    user-select: all;
  }
  .value {
    font-size: 1.35rem;
    font-weight: 600;
    letter-spacing: 0.06em;
  }
  img {
    width: 132px;
    image-rendering: pixelated;
    border-radius: 6px;
    background: #fff;
  }
  p {
    margin: 0;
  }
</style>
