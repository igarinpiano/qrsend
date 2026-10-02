<script lang="ts">
  import Home from "./views/Home.svelte";
  import SendView from "./views/SendView.svelte";
  import ReceiveView from "./views/ReceiveView.svelte";
  import DevicesView from "./views/DevicesView.svelte";
  import InboxView from "./views/InboxView.svelte";

  function parse() {
    const [path, query = ""] = location.hash.replace(/^#/, "").split("?");
    return { path: path || "/", params: new URLSearchParams(query) };
  }

  let route = $state(parse());

  $effect(() => {
    const onHash = () => (route = parse());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
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
    {:else}
      <Home />
    {/if}
  {/key}
</main>

<style>
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
  nav a.active {
    color: var(--text);
    background: var(--surface-2);
  }
</style>
