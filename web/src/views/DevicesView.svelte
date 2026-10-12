<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { IdCollector, classicDeviceId, deviceIdCodes, renderText } from "../lib/core";
  import { forget, inspect, trust, trustedDevices, type DeviceInfo, type Trusted } from "../lib/devices";
  import { canKeepKeys, createIdentity, identityInfo, loadIdentity, renameIdentity, type StoredIdentity } from "../lib/keys";
  import { toDataUrl } from "../lib/qrdraw";
  import { copyText } from "../lib/save";
  import { RELOAD_HINT, recoverFromLoadFailure } from "../lib/update";
  import Camera from "./Camera.svelte";

  let me = $state<StoredIdentity | undefined>();
  /** This device's ID as it is, and the one it shows: the same, or (for versions before post-quantum keys) its older ID. */
  let full = $state<DeviceInfo | undefined>();
  let older = $state(false);
  let mine = $state<DeviceInfo | undefined>();
  // The ID as QR codes: one for an ID of the older kind; about nine, shown one after the other, for one with
  // post-quantum keys (some 3 kB of keys). Whoever reads them may hold one, or step through them by hand.
  let codes = $state<string[]>([]);
  let showing = $state(0);
  let playing = $state(true);
  const CODE_MS = 700;
  const turn = setInterval(() => {
    if (playing && codes.length > 1) showing = (showing + 1) % codes.length;
  }, CODE_MS);
  onDestroy(() => clearInterval(turn));
  function step(by: number) {
    playing = false;
    showing = (showing + by + codes.length) % codes.length;
  }
  /** Reads an ID from the codes a camera sees, and says how far it is. */
  let collector: IdCollector | undefined;
  let scanned = $state<{ have: number; of: number } | undefined>();
  let note = $state("");
  let name = $state(guessName());
  let devices = $state<Trusted[]>([]);
  let pasted = $state("");
  let candidate = $state<DeviceInfo | undefined>();
  let candidateName = $state("");
  let scanning = $state(false);
  let error = $state("");
  let copied = $state(false);
  /** Whether this browser keeps the private keys where scripts cannot read them. */
  let protectedKeys = $state(true);

  function guessName(): string {
    const ua = navigator.userAgent;
    const os = /iPhone/.test(ua) ? "iPhone" : /iPad/.test(ua) ? "iPad" : /Android/.test(ua) ? "Android" : /Mac/.test(ua) ? "Mac" : /Windows/.test(ua) ? "Windows" : "Browser";
    return `${os} browser`;
  }

  async function load() {
    me = await loadIdentity();
    if (me) {
      full = identityInfo(me);
      show();
    }
    devices = await trustedDevices();
    protectedKeys = await canKeepKeys();
  }

  /** Shows the ID with post-quantum keys, or the one for versions before them. */
  function show() {
    if (!full) return;
    mine = older ? (classicDeviceId(full.id) as DeviceInfo) : full;
    codes = (deviceIdCodes(mine.id) as string[]).map((code) => toDataUrl(renderText(code), 5));
    showing = 0;
    playing = true;
    copied = false;
  }

  onMount(() => {
    load().catch(async (e) => {
      await recoverFromLoadFailure();
      error = `A part of the app could not be loaded (${e instanceof Error ? e.message : e}). ${RELOAD_HINT}`;
    });
  });

  async function create() {
    error = "";
    try {
      me = await createIdentity(name.trim() || guessName());
      await load();
      if (!me) error = "The device ID could not be stored in this browser (private browsing?).";
    } catch (e) {
      error = `Cannot create a device ID: ${e instanceof Error ? e.message : e}`;
    }
  }

  async function rename() {
    const next = prompt("Device name", mine?.name);
    if (me && next) {
      me = await renameIdentity(me, next);
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
      const done = await trust(candidate, candidateName.trim() || candidate.name);
      note = done === "upgraded" ? `${candidateName.trim() || candidate.name} was trusted already; it now has post-quantum keys here as well.` : "";
      candidate = undefined;
      pasted = "";
      await load();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }

  function onScan(texts: string[]) {
    if (candidate) return;
    collector ??= new IdCollector();
    for (const text of texts) {
      const read = collector.add(text) as { device?: DeviceInfo; have: number; of: number };
      if (read.device) {
        scanned = undefined;
        collector = undefined;
        check(read.device.id);
        return;
      }
      if (read.of > 1) scanned = { have: read.have, of: read.of };
    }
  }
</script>

<h2>Devices</h2>

<div class="card stack">
  <h3>This device</h3>
  {#if mine}
    <div class="me">
      <div class="codes">
        <img src={codes[showing]} alt="QR code of this device's ID" data-testid="my-id-code" data-part={showing + 1} data-parts={codes.length} />
        {#if codes.length > 1}
          <div class="row small" data-testid="my-id-turn">
            <button class="link" onclick={() => step(-1)} aria-label="Previous code">◀</button>
            <span>Code {showing + 1} of {codes.length}</span>
            <button class="link" onclick={() => step(1)} aria-label="Next code">▶</button>
            <button class="link" onclick={() => (playing = !playing)}>{playing ? "Hold" : "Go on"}</button>
          </div>
        {/if}
      </div>
      <div class="stack">
        <div><strong>{mine.name}</strong> <button class="link" onclick={rename}>Rename</button></div>
        <div data-testid="my-keys">
          {#if mine.pq}
            <span class="badge ok">post-quantum keys</span>
          {:else}
            <span class="badge warn">older keys only</span>
          {/if}
        </div>
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
    {#if full?.pq}
      <label class="check small">
        <input type="checkbox" bind:checked={older} onchange={show} />
        <span>Show the ID for versions before post-quantum keys (0.1.10 and earlier)</span>
      </label>
      {#if older}
        <p class="small warning" role="alert" data-testid="older-id-warning">
          <strong>Older ID.</strong> This is this device with its older keys only. What a device given this ID sends here
          is encrypted the older way only (no protection against a future quantum computer), and it checks this device's
          signature the older way only. Use it only for a device that cannot read the full ID, and add the full one
          there once it can.
        </p>
      {/if}
    {/if}
    {#if me?.kind === "legacy"}
      <p class="small muted">This browser cannot keep these keys in its protected key store; they are stored as site data.</p>
    {/if}
    <p class="small muted">
      To receive encrypted transfers, show {codes.length > 1 ? "these codes" : "this code"} to the sending device
      (Devices → Scan, or <code>qrsend devices add</code>) and check that it displays the same fingerprint.
      {#if codes.length > 1}
        The ID holds post-quantum keys and takes {codes.length} codes, shown one after the other; the reader takes them
        in any order.
      {/if}
    </p>
  {:else}
    <p>Create an ID so other devices can encrypt transfers for this browser and verify what you send.</p>
    <label class="field">
      <span>Device name</span>
      <input type="text" bind:value={name} />
    </label>
    <button class="primary" onclick={create}>Create device ID</button>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <p class="small muted">
      {#if protectedKeys}
        The private keys are created inside this browser and cannot be exported — not even by this page. Clearing site
        data deletes them.
      {:else}
        The private keys are created inside this browser and kept as site data (this browser cannot keep them in its
        protected key store). Clearing site data deletes them.
      {/if}
    </p>
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
          <span>
            <strong>{d.name}</strong>
            {#if d.link}
              <span class="badge ok" title="Has connected over the local network before: with “Remember trusted devices”, it connects without showing its screen.">
                connects directly
              </span>
            {/if}
            {#if d.pq}
              <span class="badge ok" title="Known with its post-quantum keys: what is sent to it is encrypted with X25519 and ML-KEM together, and its signature is checked with Ed25519 and ML-DSA together.">
                post-quantum
              </span>
            {:else}
              <span class="badge warn" title="Trusted with keys from before post-quantum ones: what is sent to it is encrypted the older way only. Have it show its ID again and add that.">
                older keys only
              </span>
            {/if}
            <br /><span class="mono small muted">{d.fingerprint}</span>
          </span>
          <button class="danger" onclick={async () => { await forget(d.fingerprint); await load(); }}>Remove</button>
        </li>
      {/each}
    </ul>
  {/if}

  {#if devices.some((d) => !d.pq)}
    <p class="small warning" role="alert" data-testid="older-devices-warning">
      <strong>Older keys in use.</strong> What is sent to a device marked “older keys only” is encrypted the older way
      only, and its signature is checked the older way only (no protection against a future quantum computer). Have it
      show its device ID again (on a current version) and add that here: its entry takes the new keys.
    </p>
  {/if}

  {#if candidate}
    <div class="confirm stack">
      <p>Does the other device show this fingerprint?</p>
      <p class="mono fpr">{candidate.fingerprint}</p>
      {#if candidate.pq}
        <p class="small"><span class="badge ok">post-quantum keys</span></p>
      {:else}
        <p class="small warning" role="alert" data-testid="older-candidate-warning">
          <span class="badge warn">older keys only</span> This ID has no post-quantum keys: what is sent to this device
          will be encrypted the older way only. A device on a current version shows an ID with them.
        </p>
      {/if}
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
      {#if scanned}
        <p class="small" data-testid="id-scan-progress">{scanned.have} of {scanned.of} codes of the ID read…</p>
      {/if}
      <button onclick={() => (scanning = false)}>Stop scanning</button>
    {:else}
      <button class="primary" onclick={() => (scanning = true)}>Scan its QR code</button>
    {/if}
    <label class="field">
      <span>…or paste its ID</span>
      <input type="text" bind:value={pasted} placeholder="qrsend-id:…" />
    </label>
    <button disabled={!pasted.trim()} onclick={() => check(pasted)}>Check</button>
  {/if}
  {#if error}<p class="error">{error}</p>{/if}
  {#if note}<p class="small" data-testid="trust-note">{note}</p>{/if}
</div>

<style>
  .me {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .codes {
    width: min(260px, 100%);
  }
  .me img {
    width: 100%;
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
