<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "$lib/api";
  import type { SystemInfo, SystemScreen, SettingNamespace } from "$lib/types";

  let { serial }: { serial: string } = $props();

  let info = $state<SystemInfo | null>(null);
  let loading = $state(false);
  let err = $state<string | null>(null);
  let busy = $state<string | null>(null);
  let message = $state("");
  let launchPkg = $state("");

  async function load() {
    loading = true;
    err = null;
    try {
      info = await api.systemInfo(serial);
    } catch (e) {
      err = String(e);
    } finally {
      loading = false;
    }
  }

  // Write a device setting, then refresh so the active choice updates.
  async function writeSetting(
    namespace: SettingNamespace,
    key: string,
    value: string,
    busyId: string,
  ) {
    busy = busyId;
    message = "";
    try {
      const r = await api.writeSetting(serial, namespace, key, value);
      message = `${key} → ${value || "(default)"}: ${r.message.trim()}`;
      info = await api.systemInfo(serial);
    } catch (e) {
      message = `${key}: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function tool(label: string, busyId: string, fn: () => Promise<{ ok: boolean; message: string }>) {
    busy = busyId;
    message = "";
    try {
      const r = await fn();
      message = `${label}: ${r.message.trim() || (r.ok ? "ok" : "failed")}`;
    } catch (e) {
      message = `${label}: ${e}`;
    } finally {
      busy = null;
    }
  }

  async function launch() {
    const pkg = launchPkg.trim();
    if (!pkg) {
      message = "Enter a package name first (e.g. com.netflix.ninja).";
      return;
    }
    await tool(`Launch ${pkg}`, "launch", () => api.launchApp(serial, pkg));
  }

  async function setPlayProtect(enabled: boolean) {
    busy = "play_protect";
    message = "";
    try {
      const r = await api.setPlayProtect(serial, enabled);
      message = `Play Protect ${enabled ? "on" : "off"}: ${r.message.trim() || "ok"}`;
      info = await api.systemInfo(serial);
    } catch (e) {
      message = `Play Protect: ${e}`;
    } finally {
      busy = null;
    }
  }

  function rotationLabel(v: string | null): string {
    return v === "0" ? "0° (landscape)" : v === "1" ? "90°" : v === "2" ? "180°" : v === "3" ? "270°" : "Unset";
  }
  function timeoutLabel(v: string | null): string {
    if (!v) return "Unset";
    const ms = Number(v);
    if (!Number.isFinite(ms)) return v;
    if (ms >= 2147483647) return "Never";
    const min = Math.round(ms / 60000);
    return min >= 1 ? `${min} min` : `${Math.round(ms / 1000)} s`;
  }
  function onOffLabel(v: string | null): string {
    return v === "1" ? "On" : v === "0" ? "Off" : "Unset";
  }
  function gpsLabel(v: string | null): string {
    return v === "0" ? "Off" : v == null ? "Unset" : "On";
  }

  const TIMEOUTS = [
    { v: "900000", label: "15 min" },
    { v: "1800000", label: "30 min" },
    { v: "3600000", label: "1 hour" },
    { v: "2147483647", label: "Never" },
  ];
  const ROTATIONS = [
    { v: "0", label: "0°" },
    { v: "1", label: "90°" },
    { v: "2", label: "180°" },
    { v: "3", label: "270°" },
  ];

  onMount(load);
</script>

<div class="card" role="tabpanel" tabindex={0} id="tabpanel-system" aria-labelledby="tab-system">
  <div class="card-header">
    <h2>System</h2>
    <button onclick={load} disabled={loading}>{loading ? "Loading…" : "Refresh"}</button>
  </div>
  <p class="muted small">
    Device settings, quick tools, and a few facts the other tabs don't show. Setting
    writes use <code>settings put</code> (Reset returns to default).
  </p>

  {#if message}
    <p class="muted small mono action-message">{message}</p>
  {/if}
  {#if err}
    <div class="error">{err}</div>
  {:else if !info}
    <div class="muted">{loading ? "Querying…" : "—"}</div>
  {:else}
    <h3>Device Info</h3>
    <dl class="kv">
      <dt>Battery</dt>
      <dd>{info.battery_percent != null ? `${info.battery_percent}%` : "No battery (mains-powered)"}</dd>
      <dt>Keyboard (IME)</dt>
      <dd class="mono small">{info.input_method ?? "—"}</dd>
    </dl>

    <h3>Play Protect</h3>
    <p class="rec">★ Recommended: <strong>On</strong> — keep Google's malware scan on for normal use. Turn it off only while sideloading apps it false-flags; the auto-installer on the Install APK tab handles this for you and restores it after.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{info.play_protect_enabled == null ? "Unset" : info.play_protect_enabled ? "On" : "Off"}</strong></div>
        <div class="muted small mono">global.package_verifier_enable</div>
      </div>
      <div class="row-actions">
        <button
          class="small-action"
          class:active={info.play_protect_enabled === true}
          class:recommended={true}
          disabled={busy === "play_protect"}
          onclick={() => setPlayProtect(true)}
        >On</button>
        <button
          class="small-action"
          class:active={info.play_protect_enabled === false}
          disabled={busy === "play_protect"}
          onclick={() => setPlayProtect(false)}
        >Off</button>
      </div>
    </div>

    <h3>Performance</h3>
    <p class="muted small">
      AOT-recompile every app from its collected profile. Speeds up app launches; runs
      for a few minutes and is safe to leave running.
    </p>
    <div class="row-actions">
      <button disabled={busy === "compile"} onclick={() => tool("Compile speed-profile", "compile", () => api.compileSpeedProfile(serial))}>
        {busy === "compile" ? "Compiling… (minutes)" : "Compile speed-profile"}
      </button>
    </div>

    <h3>Screen Rotation</h3>
    <p class="rec">★ Recommended: <strong>0° (landscape)</strong> — the normal orientation for a TV. The other angles are for unusual mounts/signage.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{rotationLabel(info.user_rotation)}</strong></div>
        <div class="muted small mono">system.user_rotation = {info.user_rotation ?? "(unset)"}</div>
      </div>
      <div class="row-actions">
        {#each ROTATIONS as opt (opt.v)}
          <button
            class="small-action"
            class:active={info.user_rotation === opt.v}
            class:recommended={opt.v === "0"}
            disabled={busy === "user_rotation"}
            onclick={() => writeSetting("system", "user_rotation", opt.v, "user_rotation")}
          >{opt.label}</button>
        {/each}
      </div>
    </div>

    <h3>Screen Timeout</h3>
    <p class="rec">★ Recommended: <strong>30 min</strong> — video apps hold the screen awake while playing, so this only governs idle time. Pick <em>Never</em> if a screensaver ever interrupts you.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{timeoutLabel(info.screen_off_timeout)}</strong></div>
        <div class="muted small mono">system.screen_off_timeout = {info.screen_off_timeout ?? "(unset)"}</div>
      </div>
      <div class="row-actions">
        {#each TIMEOUTS as opt (opt.v)}
          <button
            class="small-action"
            class:active={info.screen_off_timeout === opt.v}
            class:recommended={opt.v === "1800000"}
            disabled={busy === "screen_off_timeout"}
            onclick={() => writeSetting("system", "screen_off_timeout", opt.v, "screen_off_timeout")}
          >{opt.label}</button>
        {/each}
      </div>
    </div>

    <h3>Location (GPS)</h3>
    <p class="rec">★ Recommended: <strong>Off</strong> — a TV rarely needs location, and off removes a privacy/battery surface. Turn on only if an app genuinely needs it.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{gpsLabel(info.location_mode)}</strong></div>
        <div class="muted small mono">secure.location_mode = {info.location_mode ?? "(unset)"}</div>
      </div>
      <div class="row-actions">
        <button
          class="small-action"
          class:active={info.location_mode === "3"}
          disabled={busy === "location_mode"}
          onclick={() => writeSetting("secure", "location_mode", "3", "location_mode")}
        >On</button>
        <button
          class="small-action"
          class:active={info.location_mode === "0"}
          class:recommended={true}
          disabled={busy === "location_mode"}
          onclick={() => writeSetting("secure", "location_mode", "0", "location_mode")}
        >Off</button>
      </div>
    </div>

    <h3>Developer Options</h3>
    <p class="rec">★ Recommended: <strong>On</strong> — keeps the Developer Options menu (and the Network debugging toggle this app relies on) visible.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{onOffLabel(info.developer_options)}</strong></div>
        <div class="muted small mono">global.development_settings_enabled = {info.developer_options ?? "(unset)"}</div>
      </div>
      <div class="row-actions">
        <button
          class="small-action"
          class:active={info.developer_options === "1"}
          class:recommended={true}
          disabled={busy === "development_settings_enabled"}
          onclick={() => writeSetting("global", "development_settings_enabled", "1", "development_settings_enabled")}
        >On</button>
        <button
          class="small-action"
          class:active={info.developer_options === "0"}
          disabled={busy === "development_settings_enabled"}
          onclick={() => writeSetting("global", "development_settings_enabled", "0", "development_settings_enabled")}
        >Off</button>
      </div>
    </div>

    <h3>Ambient Display (Doze)</h3>
    <p class="muted small">Lets the device drop into a low-power doze when idle. Leave at the device default unless you have a reason to change it.</p>
    <div class="tweak-row">
      <div>
        <div class="current">Current: <strong>{onOffLabel(info.doze_enabled)}</strong></div>
        <div class="muted small mono">secure.doze_enabled = {info.doze_enabled ?? "(unset)"}</div>
      </div>
      <div class="row-actions">
        <button class="small-action" class:active={info.doze_enabled === "1"} disabled={busy === "doze_enabled"} onclick={() => writeSetting("secure", "doze_enabled", "1", "doze_enabled")}>On</button>
        <button class="small-action" class:active={info.doze_enabled === "0"} disabled={busy === "doze_enabled"} onclick={() => writeSetting("secure", "doze_enabled", "0", "doze_enabled")}>Off</button>
        <button class="small-action" disabled={busy === "doze_enabled"} onclick={() => writeSetting("secure", "doze_enabled", "", "doze_enabled")}>Reset</button>
      </div>
    </div>

    <h3>Time</h3>
    <p class="muted small">Reset the NTP server and nudge a sync — fixes the clock drift that breaks HTTPS and streaming sign-in.</p>
    <div class="row-actions">
      <button class="small-action" disabled={busy === "ntp"} onclick={() => tool("Repair NTP", "ntp", () => api.repairNtp(serial))}>Repair clock (NTP)</button>
    </div>

    <h3>Power</h3>
    <div class="row-actions">
      <button class="small-action" disabled={busy === "wake"} onclick={() => tool("Wake", "wake", () => api.powerAction(serial, true))}>Wake</button>
      <button class="small-action" disabled={busy === "sleep"} onclick={() => tool("Sleep", "sleep", () => api.powerAction(serial, false))}>Sleep</button>
    </div>

    <h3>Open on the TV</h3>
    <p class="muted small">Jump straight to a system screen on the device.</p>
    <div class="row-actions">
      {#each [
        { s: "notification_shade", label: "Notifications" },
        { s: "wifi_settings", label: "Wi-Fi" },
        { s: "bluetooth_settings", label: "Bluetooth" },
        { s: "display_settings", label: "Display" },
        { s: "app_settings", label: "Apps" },
        { s: "developer_options", label: "Developer" },
        { s: "system_updates", label: "System updates" },
      ] as item (item.s)}
        <button class="small-action subtle" disabled={busy === item.s} onclick={() => tool(item.label, item.s, () => api.openSystemScreen(serial, item.s as SystemScreen))}>{item.label}</button>
      {/each}
    </div>

    <h3>Launch App</h3>
    <p class="muted small">Open any installed app by its package name.</p>
    <div class="dns-custom">
      <input
        type="text"
        placeholder="com.netflix.ninja"
        bind:value={launchPkg}
        disabled={busy === "launch"}
        onkeydown={(e) => e.key === "Enter" && launch()}
      />
      <button class="small-action" disabled={busy === "launch"} onclick={launch}>Launch</button>
    </div>
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
  .kv {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 0.3rem 1rem;
    margin: 0.4rem 0 0.6rem;
  }
  .kv dt {
    color: var(--fg-secondary);
    font-size: 0.85rem;
  }
  .kv dd {
    margin: 0;
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
  .small-action.subtle {
    opacity: 0.92;
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
  .dns-custom {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 0.5rem;
    flex-wrap: wrap;
  }
  .dns-custom input {
    flex: 1;
    min-width: 200px;
    max-width: 320px;
  }
</style>
