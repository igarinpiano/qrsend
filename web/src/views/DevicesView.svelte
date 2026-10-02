<script lang="ts">
  import { onMount } from "svelte";
  import { renderText, type Identity } from "../lib/core";
  import {
    createIdentity,
    forget,
    inspect,
    loadIdentity,
    renameIdentity,
    trust,
    trustedDevices,
    type DeviceInfo,
    type Trusted,
  } from "../lib/devices";
  import { toDataUrl } from "../lib/qrdraw";
  import { copyText } from "../lib/save";
  import Camera from "./Camera.svelte";

  let me = $state<Identity | undefined>();
  let mine = $state<DeviceInfo | undefined>();
  let qr = $state("");
  let name = $state(guessName());
  let devices = $state<Trusted[]>([]);
  let pasted = $state("");
  let candidate = $state<DeviceInfo | undefined>();
  let candidateName = $state("");
  let scanning = $state(false);
  let error = $state("");
  let copied = $state(false);

  function guessName(): string {
    const ua = navigator.userAgent;
    const os = /iPhone/.test(ua) ? "iPhone" : /iPad/.test(ua) ? "iPad" : /Android/.test(ua) ? "Android" : /Mac/.test(ua) ? "Mac" : /Windows/.test(ua) ? "Windows" : "Browser";
    return `${os} browser`;
  }

  async function load() {
    me = await loadIdentity();
    if (me) {
      mine = me.info() as DeviceInfo;
      qr = toDataUrl(renderText(mine.id), 5);
    }
    devices = await trustedDevices();
  }

  onMount(load);

  async function create() {
    me = await createIdentity(name.trim() || guessName());
    await load();
  }

  async function rename() {
    const next = prompt("Device name", mine?.name);
    if (me && next) {
      await renameIdentity(me, next);
      await load();
    }
  }

  async function check(id: string) {
    error = "";
    try {
      candidate = await inspect(id);
      candidateName = candidate.name;
      scanning = false;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  async function confirmTrust() {
    if (!candidate) return;
    try {
      await trust(candidate, candidateName.trim() || candidate.name);
      candidate = undefined;
      pasted = "";
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function onScan(texts: string[]) {
    const id = texts.find((t) => t.startsWith("qrsend-id:"));
    if (id && !candidate) check(id);
  }
</script>

<h2>Devices</h2>

<div class="card stack">
  <h3>This device</h3>
  {#if mine}
    <div class="me">
      <img src={qr} alt="QR code of this device's ID" />
      <div class="stack">
        <div><strong>{mine.name}</strong> <button class="link" onclick={rename}>Rename</button></div>
        <div class="small">Fingerprint<br /><span class="mono fpr">{mine.fingerprint}</span></div>
        <button
          onclick={async () => {
            await copyText(mine!.id);
            copied = true;
          }}>{copied ? "Copied" : "Copy ID"}</button
        >
      </div>
    </div>
    <details>
      <summary class="small">Show ID as text</summary>
      <code data-testid="my-id">{mine.id}</code>
    </details>
    <p class="small muted">
      To receive encrypted transfers, show this code to the sending device (Devices → Scan, or
      <code>qrsend devices add</code>) and check that it displays the same fingerprint.
    </p>
  {:else}
    <p>Create an ID so other devices can encrypt transfers for this browser and verify what you send.</p>
    <label class="field">
      <span>Device name</span>
      <input type="text" bind:value={name} />
    </label>
    <button class="primary" onclick={create}>Create device ID</button>
    <p class="small muted">The keys stay in this browser. Clearing site data deletes them.</p>
  {/if}
</div>

<div class="card stack">
  <h3>Trusted devices</h3>
  {#if devices.length === 0}
    <p class="muted small">None yet. Add the devices you send to.</p>
  {:else}
    <ul class="list">
      {#each devices as d (d.fingerprint)}
        <li class="row spread">
          <span><strong>{d.name}</strong><br /><span class="mono small muted">{d.fingerprint}</span></span>
          <button class="danger" onclick={async () => { await forget(d.fingerprint); await load(); }}>Remove</button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if candidate}
    <div class="confirm stack">
      <p>Does the other device show this fingerprint?</p>
      <p class="mono fpr">{candidate.fingerprint}</p>
      <label class="field">
        <span>Name</span>
        <input type="text" bind:value={candidateName} />
      </label>
      <div class="row">
        <button class="primary" onclick={confirmTrust}>Yes, trust it</button>
        <button onclick={() => (candidate = undefined)}>Cancel</button>
      </div>
    </div>
  {:else}
    <h3>Add a device</h3>
    {#if scanning}
      <Camera ontexts={onScan} />
      <button onclick={() => (scanning = false)}>Stop scanning</button>
    {:else}
      <button class="primary" onclick={() => (scanning = true)}>Scan its QR code</button>
    {/if}
    <label class="field">
      <span>…or paste its ID</span>
      <input type="text" bind:value={pasted} placeholder="qrsend-id:1:age1…" />
    </label>
    <button disabled={!pasted.trim()} onclick={() => check(pasted)}>Check</button>
  {/if}
  {#if error}<p class="error">{error}</p>{/if}
</div>

<style>
  .me {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .me img {
    width: min(220px, 100%);
    image-rendering: pixelated;
    border-radius: 8px;
    background: #fff;
  }
  .fpr {
    font-size: 1.2rem;
    letter-spacing: 0.04em;
  }
  .link {
    min-height: 0;
    padding: 2px 6px;
    font-weight: 500;
    font-size: 0.85rem;
  }
  .confirm {
    border-top: 1px solid var(--border);
    padding-top: 12px;
  }
</style>
