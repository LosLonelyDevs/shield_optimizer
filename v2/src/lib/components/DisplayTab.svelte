<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "$lib/api";
  import type { DisplayReport, MediaApps } from "$lib/types";

  let { serial }: { serial: string } = $props();

  let report = $state<DisplayReport | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let actionBusy = $state<string | null>(null);
  let actionMessage = $state("");

  let media = $state<MediaApps | null>(null);
  let grantBusy = $state(false);
  let grantMessage = $state("");

  async function load() {
    loading = true;
    err = null;
    try {
      const [r, m] = await Promise.all([
        api.displayReport(serial),
        api.detectMediaApps(serial).catch(() => null),
      ]);
      report = r;
      media = m;
    } catch (e) {
      err = String(e);
    } finally {
      loading = false;
    }
  }

  function matchContentLabel(v: string | null): string {
    return v === "0" ? "Never" : v === "1" ? "Seamless only" : v === "2" ? "Always" : "Unset (default)";
  }

  async function setMatchContent(value: string) {
    actionBusy = "mcfr";
    actionMessage = "";
    try {
      const r = await api.writeSetting(serial, "secure", "match_content_frame_rate", value);
      actionMessage = `Match-content frame rate → ${value || "(default)"}: ${r.message.trim()}`;
      report = await api.displayReport(serial);
    } catch (e) {
      actionMessage = `Match-content frame rate: ${e}`;
    } finally {
      actionBusy = null;
    }
  }

  async function grantRefreshRate() {
    const pkg = media?.refresh_rate_app;
    if (!pkg) return;
    grantBusy = true;
    grantMessage = "";
    try {
      const r = await api.grantWriteSecureSettings(serial, pkg);
      grantMessage = r.message.trim() || (r.ok ? "Granted." : "Failed.");
    } catch (e) {
      grantMessage = `Grant: ${e}`;
    } finally {
      grantBusy = false;
    }
  }

  onMount(load);
</script>

<div class="card" role="tabpanel" tabindex={0} id="tabpanel-display" aria-labelledby="tab-display">
  <div class="card-header">
    <h2>Display</h2>
    <button onclick={load} disabled={loading}>{loading ? "Loading…" : "Refresh"}</button>
  </div>
  <p class="muted small">
    The panel's modes plus the one display setting ADB can change. The base HDMI
    output mode (4K60 vs 59.94, Dolby Vision, color space) is NVIDIA-UI-only — see
    the note at the bottom.
  </p>

  {#if err}
    <div class="error">{err}</div>
  {:else if !report}
    <div class="muted">{loading ? "Querying…" : "—"}</div>
  {:else}
    {#if actionMessage}
      <p class="muted small mono action-message">{actionMessage}</p>
    {/if}

    <h3>Current Mode</h3>
    <div class="current-scaling muted small mono">
      Resolution: {report.current.resolution ?? "unknown"}
      <br />
      Refresh: {report.current.refresh_hz != null ? `${report.current.refresh_hz} Hz` : "unknown"}
      <br />
      HDR: {report.current.hdr_types.length ? report.current.hdr_types.join(", ") : "SDR only"}
    </div>

    <h3>Supported Modes</h3>
    <p class="muted small">
      Modes the panel advertises (read-only — Android TV exposes no ADB way to force
      a base output mode; this confirms what's available, e.g. that 4K60 exists).
    </p>
    {#if report.supported_modes.length}
      <div class="mode-grid">
        {#each report.supported_modes as m (`${m.resolution}@${m.refresh_hz}`)}
          <div class="mode-chip" class:active={m.active}>
            <span class="mono">{m.resolution}</span>
            <span class="muted small">{m.refresh_hz} Hz{m.active ? " · active" : ""}</span>
          </div>
        {/each}
      </div>
    {:else}
      <div class="muted small">No modes parsed from <code>dumpsys display</code>.</div>
    {/if}

    <h3>Match Content Frame Rate</h3>
    <p class="muted small">
      Lets apps switch the panel to match video fps (24/25/30/50/60). Seamless avoids
      a black flash on the switch. Captured in snapshots, so it's reversible.
    </p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{matchContentLabel(report.match_content_frame_rate)}</strong></div>
        <div class="muted small mono">secure.match_content_frame_rate = {report.match_content_frame_rate ?? "(unset)"}</div>
      </div>
      <div class="row-actions">
        {#each [{ v: "0", label: "Never" }, { v: "1", label: "Seamless only" }, { v: "2", label: "Always" }] as opt (opt.v)}
          <button
            class="small-action"
            class:active={report.match_content_frame_rate === opt.v}
            disabled={actionBusy === "mcfr"}
            onclick={() => setMatchContent(opt.v)}
          >{opt.label}</button>
        {/each}
        <button class="small-action" disabled={actionBusy === "mcfr"} onclick={() => setMatchContent("")}>Reset</button>
      </div>
    </div>

    <h3>Per-App Refresh Rate</h3>
    <p class="muted small">
      A dedicated app can switch the Shield's display mode per streaming app on launch
      (e.g. 4K@24 for Netflix). It needs <code>WRITE_SECURE_SETTINGS</code>, which ADB
      can grant. The per-app profiles live in the app's own storage, so set those in
      the app itself.
    </p>
    {#if media?.refresh_rate_app}
      <div class="tweak-row">
        <div>
          <div class="current">Detected: <strong>{media.refresh_rate_app}</strong></div>
          <div class="muted small">Grant the permission, then configure profiles in the app.</div>
        </div>
        <div class="row-actions">
          <button class="small-action" disabled={grantBusy} onclick={grantRefreshRate}>Grant WRITE_SECURE_SETTINGS</button>
        </div>
      </div>
      {#if grantMessage}<p class="muted small mono action-message">{grantMessage}</p>{/if}
    {:else}
      <div class="muted small">
        No per-app refresh-rate app detected. Install one (e.g. "Refresh Rate") from the
        Play Store, then come back to grant the permission.
      </div>
    {/if}
    <details class="guide">
      <summary>Recommended per-app profiles</summary>
      <ul class="muted small">
        <li>Netflix / Disney+ / HBO Max / Prime Video → <strong>4K @ 24 Hz</strong></li>
        <li>European broadcast apps → <strong>4K @ 50 Hz</strong></li>
        <li>BBC iPlayer → needs the Shield base mode at <strong>25/50 Hz</strong></li>
        <li>Plex / Kodi / GeForce Now / Moonlight → <strong>skip</strong> (native handling)</li>
      </ul>
    </details>

    <h3>Set on the device (not ADB-settable)</h3>
    <p class="muted small">
      NVIDIA UI-only — no ADB key exists. Set these under
      <em>Settings → Device Preferences → Display &amp; Sound</em>:
    </p>
    <ul class="muted small oos">
      <li>Base resolution / refresh (prefer 4K 60 Hz over 59.94 where offered)</li>
      <li>Dolby Vision mode / Low-Latency Dolby Vision (developer option)</li>
      <li>Match content color space / colorimetry</li>
      <li>ALLM / Automatic Game Mode and its app list</li>
    </ul>
  {/if}
</div>

<style>
  .card {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1.2rem;
  }
  .card h2 {
    margin: 0 0 0.8rem;
    font-size: 1.1rem;
  }
  .card h3 {
    margin: 1rem 0 0.4rem;
    font-size: 1rem;
    color: var(--fg-secondary);
  }
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .small {
    font-size: 0.82rem;
  }
  .mono {
    font-family: ui-monospace, monospace;
  }
  .error {
    background: var(--danger-surface);
    color: var(--danger-text);
    padding: 0.7rem 1rem;
    border-radius: 6px;
    font-family: ui-monospace, monospace;
    font-size: 0.85rem;
  }
  .row-actions {
    display: flex;
    gap: 0.4rem;
    align-items: center;
    flex-wrap: wrap;
  }
  .small-action {
    padding: 0.2rem 0.6rem;
    font-size: 0.78rem;
  }
  .small-action.active {
    background: var(--accent-strong);
    color: #fff;
    border-color: var(--accent);
  }
  .current {
    font-size: 0.85rem;
    color: var(--fg-secondary);
  }
  .current strong {
    color: var(--fg-primary);
  }
  .action-message {
    margin-top: 0.4rem;
    padding: 0.4rem 0.6rem;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: 4px;
    word-break: break-word;
  }
  code {
    background: var(--bg-inset);
    border: 1px solid var(--border);
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    font-family: ui-monospace, monospace;
    font-size: 0.85em;
  }
  .tweak-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    padding: 0.5rem 0;
    border-bottom: 1px solid var(--bg-button);
  }
  .current-scaling {
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.5rem 0.7rem;
    margin: 0.4rem 0 0.6rem;
    line-height: 1.5;
  }
  .mode-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 0.4rem;
    margin: 0.4rem 0 0.6rem;
  }
  .mode-chip {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    padding: 0.4rem 0.6rem;
    background: var(--bg-button);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .mode-chip.active {
    border-color: var(--accent);
    background: var(--bg-inset);
  }
  .guide {
    margin: 0.4rem 0 0.6rem;
  }
  .guide summary {
    cursor: pointer;
    font-size: 0.85rem;
    color: var(--fg-secondary);
  }
  .guide ul,
  .oos {
    margin: 0.4rem 0;
    padding-left: 1.2rem;
    line-height: 1.6;
  }
</style>
