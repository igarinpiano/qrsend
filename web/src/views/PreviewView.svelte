<script lang="ts">
  import { PREVIEW_FEATURES, featureChoice, setFeature, type FeatureId, type PreviewFeature } from "../lib/prefs";

  const features: readonly PreviewFeature[] = PREVIEW_FEATURES;
  /** Per feature: "" when off, otherwise the way it is on. */
  let chosen = $state(Object.fromEntries(features.map((f) => [f.id, featureChoice(f.id as FeatureId)])) as Record<string, string>);

  function choose(id: string, value: string) {
    chosen[id] = value;
    setFeature(id as FeatureId, value);
  }
</script>

<h2>Feature preview</h2>
<p class="muted">
  New ways of transferring that are still being worked on. Everything here is off unless you turn it on; with all of it
  off, QRSend works exactly as before. The choice is remembered in this browser.
</p>

{#each features as feature (feature.id)}
  <div class="card stack">
    {#if feature.choices}
      <!-- The system's own menu: it is what people know on their device. The
           name comes first and keeps its room; the menu is as wide as its
           longest choice, never wider than the card, and moves below the name
           where the two do not fit side by side. -->
      <div class="feature choice">
        <span class="name">
          <strong>{feature.title}</strong>
          <span class="small muted">when {feature.side}</span>
        </span>
        <select aria-label={feature.title} value={chosen[feature.id]} onchange={(e) => choose(feature.id, e.currentTarget.value)}>
          <option value="">Off</option>
          {#each feature.choices as choice (choice.value)}
            <option value={choice.value}>{choice.label}</option>
          {/each}
        </select>
      </div>
    {:else}
      <label class="feature">
        <input type="checkbox" checked={chosen[feature.id] !== ""} onchange={(e) => choose(feature.id, e.currentTarget.checked ? "1" : "")} />
        <span>
          <strong>{feature.title}</strong>
          <span class="badge">{chosen[feature.id] ? "On" : "Off"}</span>
          <span class="small muted">when {feature.side}</span>
        </span>
      </label>
    {/if}
    <p class="small">{feature.summary}</p>
    <ol class="small muted">
      {#each feature.steps as step}
        <li>{step}</li>
      {/each}
    </ol>
  </div>
{/each}

<p class="small muted">Previews can change or disappear. A receiver that does not know a preview feature still receives the transfer the usual way.</p>

<style>
  .feature {
    display: flex;
    gap: 10px;
    align-items: center;
    font-size: 1.05rem;
  }
  .feature input {
    flex: none;
    width: 1.2em;
    height: 1.2em;
  }
  /* (Whatever is beside the control may shrink and wrap; it never pushes
     anything out of the card.) */
  .feature > span {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .choice {
    flex-wrap: wrap;
    gap: 8px 12px;
  }
  .choice .name {
    flex: 1 1 12em;
  }
  .choice select {
    /* Not the full width form fields have elsewhere. */
    flex: 0 1 auto;
    width: auto;
    max-width: 100%;
    min-width: 0;
    /* (Tall enough for a finger.) */
    min-height: 44px;
    font: inherit;
  }
  ol {
    margin: 0;
    padding-left: 1.4em;
  }
</style>
