<script lang="ts">
  import Home from "./views/Home.svelte";
  import SendView from "./views/SendView.svelte";
  import ReceiveView from "./views/ReceiveView.svelte";
  import DevicesView from "./views/DevicesView.svelte";
  import InboxView from "./views/InboxView.svelte";
  import PreviewView from "./views/PreviewView.svelte";
  import { log, logLines, logText } from "./lib/log";
  import { reloadForUpdate } from "./lib/update";

  function parse() {
    const [path, query = ""] = location.hash.replace(/^#/, "").split("?");
    return { path: path || "/", params: new URLSearchParams(query) };
  }

  let route = $state(parse());
  $effect(() => log("app", "page", { path: route.path }));

  // The diagnostic log, for pasting into a bug report. Where the clipboard
  // is out of reach, it is shown instead, to be selected by hand.
  let logCopied = $state("");
  let logShown = $state("");
  async function copyLog() {
    const text = logText();
    try {
      await navigator.clipboard.writeText(text);
      logCopied = `Copied (${logLines()} lines)`;
      logShown = "";
    } catch {
      logCopied = "Select and copy it below";
      logShown = text;
    }
    setTimeout(() => (logCopied = ""), 4000);
  }

  $effect(() => {
    const onHash = () => (route = parse());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  });

  // A new version has been installed while this page was open. The page
  // still runs the old one, whose files the new version has replaced, so it
  // should be reloaded — at once where nothing is lost by that, on request
  // where a transfer or a filled-in form may be open.
  let updateReady = $state(false);
  $effect(() => {
    const sw = navigator.serviceWorker;
    if (!sw) return;
    const controlled = !!sw.controller;
    const onChange = () => {
      // The very first install takes control too; that is no update.
      if (!controlled) return;
      if (route.path === "/send" || route.path === "/receive") updateReady = true;
      else reloadForUpdate();
    };
    sw.addEventListener("controllerchange", onChange);
    return () => sw.removeEventListener("controllerchange", onChange);
  });

  const nav = [
    { href: "#/send", label: "Send" },
    { href: "#/receive", label: "Receive" },
    { href: "#/inbox", label: "Inbox" },
    { href: "#/devices", label: "Devices" },
  ];
</script>

<header>
  <a class="brand" href="#/">
    <img src="./icon.svg" alt="" width="28" height="28" />
    QRSend
  </a>
  <nav>
    {#each nav as item}
      <a href={item.href} class:active={route.path === item.href.slice(1)}>{item.label}</a>
    {/each}
  </nav>
</header>

<main>
  {#key route.path + route.params.toString()}
    {#if route.path === "/send"}
      <SendView params={route.params} />
    {:else if route.path === "/receive"}
      <ReceiveView params={route.params} />
    {:else if route.path === "/devices"}
      <DevicesView />
    {:else if route.path === "/inbox"}
      <InboxView />
    {:else if route.path === "/preview"}
      <PreviewView />
    {:else}
      <Home />
    {/if}
  {/key}
</main>

{#if updateReady}
  <div class="update" role="status">
    <span>A new version of QRSend is ready.</span>
    <button class="primary" onclick={() => location.reload()}>Reload</button>
  </div>
{/if}

<footer class="small muted">
  <p>
    <a href="https://github.com/igarinpiano/qrsend">QRSend</a> is open source (Apache-2.0). Nothing you send or receive
    leaves your devices. <a href="#/preview">Feature preview</a> ·
    <button class="link" onclick={copyLog} data-testid="copy-log" title="What happened when, without file names, contents, IDs or addresses: for a bug report">
      {logCopied || "Copy log"}
    </button>
  </p>
  {#if logShown}
    <textarea class="log" readonly rows="8" data-testid="log-text" onfocus={(e) => e.currentTarget.select()}>{logShown}</textarea>
  {/if}
  <p>QR Code is a registered trademark of DENSO WAVE INCORPORATED in Japan and in other countries.</p>
</footer>

<style>
  .update {
    position: sticky;
    bottom: 12px;
    z-index: 20;
    display: flex;
    gap: 12px;
    align-items: center;
    justify-content: space-between;
    margin: 12px 16px;
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
    box-shadow: 0 4px 16px rgb(0 0 0 / 0.25);
  }
  header {
    position: sticky;
    top: 0;
    z-index: 10;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 10px 16px;
    padding-top: max(10px, env(safe-area-inset-top));
    background: color-mix(in srgb, var(--bg) 85%, transparent);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--border);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 700;
    color: var(--text);
    text-decoration: none;
  }
  nav {
    display: flex;
    gap: 4px;
    overflow-x: auto;
  }
  nav a {
    padding: 8px 10px;
    border-radius: 8px;
    color: var(--muted);
    text-decoration: none;
    font-weight: 600;
    font-size: 0.95rem;
    white-space: nowrap;
  }
  footer {
    max-width: 760px;
    margin: 0 auto;
    padding: 0 16px 32px;
  }
  footer p {
    margin: 0 0 4px;
  }
  .log {
    width: 100%;
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
  }
  nav a.active {
    color: var(--text);
    background: var(--surface-2);
  }
</style>
