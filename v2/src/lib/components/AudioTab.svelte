<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "$lib/api";
  import type { AudioReport, MediaApps, SurroundMode, KodiProfile, AdjustRefreshRate } from "$lib/types";

  let { serial }: { serial: string } = $props();

  let report = $state<AudioReport | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let message = $state("");

  // Formats the receiver actually reports it can decode. The labels match each
  // codec toggle's `f.label`, so this drives the ★ recommendation below. Empty
  // on devices (e.g. Shield over a TV/eARC path) that don't expose sink caps.
  const recommendedFormats = $derived(new Set(report?.supported_encodings ?? []));

  let media = $state<MediaApps | null>(null);
  let kodiBusy = $state(false);
  let kodiMessage = $state("");
  let kodi = $state<KodiProfile>({
    adjust_refresh_rate: "always",
    sync_playback_to_display: true,
    hdr_display: true,
    passthrough: true,
    ac3_passthrough: true,
    eac3_passthrough: true,
    dts_passthrough: true,
    truehd_passthrough: true,
    dtshd_passthrough: true,
    ac3_transcode: false,
    resolution_whitelist: [],
    whitelist_pulldown: false,
    whitelist_double_refresh: false,
  });

  const REFRESH_OPTIONS: { v: AdjustRefreshRate; label: string }[] = [
    { v: "off", label: "Off" },
    { v: "always", label: "Always" },
    { v: "on_start_stop", label: "Start/Stop" },
    { v: "on_start", label: "On start" },
  ];

  async function load() {
    loading = true;
    err = null;
    try {
      const [r, m] = await Promise.all([
        api.audioReport(serial),
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

  function modeLabel(m: SurroundMode | null): string {
    return m === "auto"
      ? "Auto"
      : m === "never"
        ? "Never (stereo)"
        : m === "always"
          ? "Always"
          : m === "manual"
            ? "Manual"
            : "Unset";
  }

  async function setMode(mode: SurroundMode) {
    busy = "mode";
    message = "";
    try {
      const r = await api.setSurroundOutput(serial, mode);
      message = r.message.trim();
      report = await api.audioReport(serial);
    } catch (e) {
      message = `Surround mode: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function toggleFormat(id: number) {
    if (!report) return;
    const enabled = report.formats.filter((f) => f.enabled).map((f) => f.id);
    const next = enabled.includes(id) ? enabled.filter((x) => x !== id) : [...enabled, id];
    busy = `fmt:${id}`;
    message = "";
    try {
      const r = await api.setSurroundFormats(serial, next);
      message = r.message.trim();
      report = await api.audioReport(serial);
    } catch (e) {
      message = `Formats: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function generateKodi() {
    kodiBusy = true;
    kodiMessage = "";
    try {
      const r = await api.stageKodiConfig(serial, kodi);
      kodiMessage = r.ok
        ? `Wrote ${r.device_path}. In Kodi → Settings → File Manager, copy it into …/.kodi/userdata/, then restart Kodi.`
        : r.message;
    } catch (e) {
      kodiMessage = `Kodi config: ${e}`;
    } finally {
      kodiBusy = false;
    }
  }

  onMount(load);
</script>

<div class="card" role="tabpanel" tabindex={0} id="tabpanel-audio" aria-labelledby="tab-audio">
  <div class="card-header">
    <h2>Audio</h2>
    <button onclick={load} disabled={loading}>{loading ? "Loading…" : "Refresh"}</button>
  </div>
  <p class="muted small">
    Control HDMI surround passthrough via <code>encoded_surround_output</code>. These
    are reversible (captured in snapshots), but <strong>Always</strong> or a format your
    receiver lacks can cause silence — use the receiver's own report below to decide.
  </p>

  {#if err}
    <div class="error">{err}</div>
  {:else if !report}
    <div class="muted">{loading ? "Querying…" : "—"}</div>
  {:else}
    {#if message}
      <p class="muted small mono action-message">{message}</p>
    {/if}

    <h3>Surround Output Mode</h3>
    <p class="rec">★ Recommended: <strong>Auto</strong> — sends exactly the formats your TV/receiver reports it can decode, so surround works without risking silence. Use <em>Manual</em> only if a codec is missing from the report but your gear truly supports it; <em>Never</em> forces safe stereo.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{modeLabel(report.mode)}</strong></div>
        <div class="muted small mono">
          global.encoded_surround_output = {report.mode_raw ?? "(unset)"}
          {#if report.active_device}· out: {report.active_device}{/if}
        </div>
      </div>
      <div class="row-actions">
        {#each [{ v: "auto", label: "Auto" }, { v: "manual", label: "Manual" }, { v: "always", label: "Always" }, { v: "never", label: "Never" }] as opt (opt.v)}
          <button
            class="small-action"
            class:active={report.mode === opt.v}
            class:recommended={opt.v === "auto"}
            disabled={busy === "mode"}
            onclick={() => setMode(opt.v as SurroundMode)}
          >{opt.label}</button>
        {/each}
      </div>
    </div>
    <p class="muted small">
      <strong>Auto</strong> sends what the receiver reports. <strong>Manual</strong> sends
      only the codecs you tick below. <strong>Never</strong> forces stereo (the safe
      sanity mode). Hit <strong>Auto</strong> to reset.
    </p>

    <h3>Codecs {#if report.mode !== "manual"}<span class="muted small">(apply in Manual mode)</span>{/if}</h3>
    {#if recommendedFormats.size > 0}
      <p class="rec">★ Your receiver reports the ★-marked formats below — switch Surround mode to <strong>Manual</strong> and enable those.</p>
    {:else}
      <p class="rec">
        No formats detected from your receiver, so there's nothing to recommend from. On a
        <strong>Shield → TV → eARC → AVR</strong> path the Shield reads the <em>TV's</em> audio
        capabilities, not the AVR's. To fix: set the TV's digital audio output to
        <strong>Bitstream/Passthrough</strong> and enable <strong>eARC</strong>, then reboot the
        Shield — or plug the Shield <strong>directly into the AVR</strong>. On eARC you can safely
        set Surround mode to <strong>Manual</strong> and enable all six below; eARC carries them.
      </p>
    {/if}
    <div class="format-grid">
      {#each report.formats as f (f.id)}
        {@const rec = recommendedFormats.has(f.label)}
        <button
          class="small-action fmt"
          class:active={f.enabled}
          class:is-rec={rec}
          disabled={busy === `fmt:${f.id}`}
          onclick={() => toggleFormat(f.id)}
        >
          <span>{f.label}{rec ? " ★" : ""}</span>
          <span class="muted small">{f.enabled ? "on" : "off"}</span>
        </button>
      {/each}
    </div>

    <h3>Receiver-Reported Formats</h3>
    {#if report.supported_encodings.length}
      <div class="muted small">{report.supported_encodings.join(" · ")}</div>
    {:else}
      <div class="muted small">
        Nothing reported — the Shield isn't receiving your receiver's audio capabilities
        (see the note above). This is common on Shield over a TV/eARC path and isn't a
        per-app limitation; Manual mode works regardless.
      </div>
    {/if}

    {#if media?.kodi}
      <h3>Kodi — Generate advancedsettings.xml</h3>
      <p class="muted small">
        Build a correct <code>advancedsettings.xml</code> and stage it to
        <code>/sdcard</code>. Android 11 blocks writing Kodi's app folder directly, so
        you import it once in Kodi (instructions appear after generating).
      </p>
      <div class="kodi-form">
        <div class="kodi-row">
          <span>Refresh-rate switching</span>
          <div class="row-actions">
            {#each REFRESH_OPTIONS as opt (opt.v)}
              <button
                class="small-action"
                class:active={kodi.adjust_refresh_rate === opt.v}
                onclick={() => (kodi.adjust_refresh_rate = opt.v)}
              >{opt.label}</button>
            {/each}
          </div>
        </div>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.sync_playback_to_display} /><span>Sync playback to display clock</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.hdr_display} /><span>Use display HDR / Dolby Vision capability</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.passthrough} /><span>Audio passthrough (master)</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.ac3_passthrough} /><span>AC-3 passthrough</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.eac3_passthrough} /><span>E-AC-3 (DD+) passthrough</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.dts_passthrough} /><span>DTS passthrough</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.truehd_passthrough} /><span>Dolby TrueHD passthrough</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.dtshd_passthrough} /><span>DTS-HD passthrough</span></label>
        <label class="kodi-check"><input type="checkbox" bind:checked={kodi.ac3_transcode} /><span>Transcode to AC-3 (for ARC / optical)</span></label>
      </div>
      <div class="row-actions">
        <button disabled={kodiBusy} onclick={generateKodi}>
          {kodiBusy ? "Generating…" : "Generate & stage to /sdcard"}
        </button>
      </div>
      {#if kodiMessage}<p class="muted small mono action-message">{kodiMessage}</p>{/if}
    {/if}

    {#if media?.plex}
      <h3>Plex</h3>
      <p class="muted small">
        Plex's playback prefs are app-internal (root-only), so they can't be set over
        ADB. Configure in Plex → Settings:
      </p>
      <ul class="muted small oos">
        <li>Audio → set passthrough to match your output (HDMI vs Optical)</li>
        <li>Video → enable refresh-rate matching</li>
        <li>Quality → prefer Direct Play / Direct Stream</li>
      </ul>
    {/if}

    <h3>Set on the device (not ADB-settable)</h3>
    <p class="muted small">
      NVIDIA-proprietary — no ADB key. Set under
      <em>Settings → Device Preferences → Display &amp; Sound → Advanced sound</em>:
    </p>
    <ul class="muted small oos">
      <li>Dolby processing</li>
      <li>Night listening (dynamic range / volume leveling)</li>
      <li>High-resolution audio</li>
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
  .small-action.recommended {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .small-action.recommended::before {
    content: "★ ";
    color: var(--accent);
  }
  .small-action.recommended.active::before {
    color: #fff;
  }
  .rec {
    font-size: 0.82rem;
    color: var(--accent);
    margin: 0.15rem 0 0.45rem;
    line-height: 1.45;
  }
  .rec strong {
    color: var(--accent);
    font-weight: 600;
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
  .format-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 0.4rem;
    margin: 0.4rem 0 0.6rem;
  }
  .fmt {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
    text-align: left;
  }
  /* Receiver reports it can decode this codec — recommended to enable. Distinct
     from .active (currently on): a recommended codec may still be off. */
  .fmt.is-rec {
    border-color: var(--accent);
    box-shadow: inset 0 0 0 1px var(--accent);
  }
  .kodi-form {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin: 0.4rem 0 0.6rem;
  }
  .kodi-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .kodi-check {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .oos {
    margin: 0.4rem 0;
    padding-left: 1.2rem;
    line-height: 1.6;
  }
</style>
