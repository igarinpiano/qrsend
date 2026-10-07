<script lang="ts">
  import { PREVIEW_FEATURES, featureOn, setFeature, type FeatureId } from "../lib/prefs";

  let on = $state(Object.fromEntries(PREVIEW_FEATURES.map((f) => [f.id, featureOn(f.id)])) as Record<FeatureId, boolean>);

  function toggle(id: FeatureId, value: boolean) {
    on[id] = value;
    setFeature(id, value);
  }
</script>

<h2>Feature preview</h2>
<p class="muted">
  New ways of sending that are still being worked on. Everything here is off unless you turn it on; with all of it off,
  QRSend sends exactly as before. The choice is remembered in this browser.
</p>

{#each PREVIEW_FEATURES as feature (feature.id)}
  <div class="card stack">
    <label class="feature">
      <input type="checkbox" checked={on[feature.id]} onchange={(e) => toggle(feature.id, e.currentTarget.checked)} />
      <span>
        <strong>{feature.title}</strong>
        <span class="badge">{on[feature.id] ? "On" : "Off"}</span>
      </span>
    </label>
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
  ol {
    margin: 0;
    padding-left: 1.4em;
  }
</style>
