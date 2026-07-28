<script lang="ts">
  import { onMount } from "svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api } from "$lib/api";
  import type { DiscoveredApk, InstalledVersion, QuickAppRow, ShizukuStatus } from "$lib/types";

  let { serial }: { serial: string } = $props();

  /// Path of the APK currently installing (null when idle) — per-path so a
  /// multi-APK list only shows the spinner on the row actually installing.
  let sideloadBusy = $state<string | null>(null);
  let sideloadResult = $state<string>("");
  let sideloadHint = $state<string | null>(null);
  /// Path the current install result belongs to (so it renders under that
  /// row), whether it succeeded, and the raw adb output for the details line.
  let sideloadResultPath = $state<string | null>(null);
  let sideloadOk = $state(false);
  // Auto-discovered APK list — re-scanned whenever the user picks a folder
  // (or after a successful install in case files were added/removed).
  let discoveredApks = $state<DiscoveredApk[]>([]);
  let discoveredFolder = $state<string | null>(null);
  let discoveryBusy = $state(false);
  /// package id → state, for the discovered APKs, so each row can say whether
  /// it's already installed on this device.
  let apkInstallState = $state<Record<string, "enabled" | "disabled" | "missing">>({});
  /// package id → the version currently installed on the device, for packages
  /// that are installed. Compared against the APK's own version so a row can say
  /// Upgrade / Downgrade / Reinstall instead of a flat "installed".
  let apkInstalledVersions = $state<Record<string, InstalledVersion>>({});

  // Quick-install catalog (arch-aware auto-download).
  let quickApps = $state<QuickAppRow[]>([]);
  let quickBusy = $state<string | null>(null);
  let quickMsg = $state("");
  let quickMsgPkg = $state<string | null>(null);
  let quickOk = $state(false);

  let shizukuBusy = $state(false);
  let shizukuMsg = $state("");
  let shizukuOk = $state(false);
  // Live install/running state for the status dot. null until the first probe
  // (or if a probe fails) — rendered as "unknown" rather than a false negative.
  let shizukuStatus = $state<ShizukuStatus | null>(null);
  let shizukuCheckBusy = $state(false);
  let shizukuStatusLabel = $derived(
    shizukuStatus === null
      ? shizukuCheckBusy
        ? "Checking…"
        : "Status unknown"
      : shizukuStatus.running
        ? "Service running"
        : shizukuStatus.installed
          ? "Installed · service stopped"
          : "Not installed",
  );

  async function pickAndInstallApk() {
    const selected = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Android Packages", extensions: ["apk"] }],
    });
    if (!selected || Array.isArray(selected)) return;
    // Remember the folder the user picked from so we can show the
    // surrounding APKs as a quick-pick list.
    const lastSep = Math.max(selected.lastIndexOf("/"), selected.lastIndexOf("\\"));
    if (lastSep > 0) {
      const folder = selected.slice(0, lastSep);
      localStorage.setItem("shieldopt.lastApkFolder", folder);
      await scanApkFolder(folder);
    }
    await installApkPath(selected);
  }

  async function pickApkFolder() {
    const picked = await openDialog({ multiple: false, directory: true });
    if (!picked || Array.isArray(picked)) return;
    localStorage.setItem("shieldopt.lastApkFolder", picked);
    await scanApkFolder(picked);
  }

  async function scanApkFolder(folder: string) {
    discoveryBusy = true;
    try {
      discoveredApks = await api.listApksInFolder(folder);
      discoveredFolder = folder;
      await refreshInstallState();
    } catch (e) {
      sideloadResult = `Scan failed: ${e}`;
    } finally {
      discoveryBusy = false;
    }
  }

  /// Re-query install state + installed versions for the currently discovered
  /// APKs, without re-listing the folder. Run on scan and again after a
  /// successful install so a row's chip reflects the new version right away.
  async function refreshInstallState() {
    const pkgs = discoveredApks.map((a) => a.package).filter((p): p is string => !!p);
    apkInstallState = pkgs.length ? await api.packageStates(serial, pkgs) : {};
    // Versions only matter for packages actually on the device.
    const installedPkgs = pkgs.filter(
      (p) => apkInstallState[p] === "enabled" || apkInstallState[p] === "disabled",
    );
    apkInstalledVersions = installedPkgs.length
      ? await api.installedPackageVersions(serial, installedPkgs)
      : {};
  }

  async function installApkPath(path: string) {
    sideloadBusy = path;
    sideloadResultPath = path;
    sideloadOk = false;
    sideloadResult = "";
    sideloadHint = null;
    try {
      const r = await api.installApk(serial, path, true);
      sideloadOk = r.ok;
      // Friendly summary; the raw adb output is kept for the details line.
      sideloadResult = r.ok
        ? "Installed."
        : installFailureSummary(r.message);
      sideloadHint = r.hint;
      // Refresh so the row's chip flips to the newly-installed version (best
      // effort — a failure here shouldn't clobber the success message).
      if (r.ok && discoveredFolder) {
        try {
          await refreshInstallState();
        } catch {
          // Keep the "Installed." result even if the re-query fails.
        }
      }
    } catch (e) {
      sideloadResult = String(e);
    } finally {
      sideloadBusy = null;
    }
  }

  /// Turn raw `adb install` failure output into a one-line summary. The full
  /// text still shows in the details line; this is the headline.
  function installFailureSummary(raw: string): string {
    const m = raw.match(/INSTALL_FAILED_[A-Z_]+|INSTALL_PARSE_FAILED[A-Z_]*/);
    if (m) {
      if (m[0].includes("ALREADY_EXISTS")) return "Already installed (same version).";
      if (m[0].includes("VERSION_DOWNGRADE")) return "A newer version is already installed.";
      if (m[0].includes("NO_MATCHING_ABIS")) return "Wrong CPU architecture for this device.";
      if (m[0].includes("OLDER_SDK")) return "Needs a newer Android version than this device.";
      return `Install failed (${m[0]}).`;
    }
    return "Install failed.";
  }

  /// The install status of one discovered APK, comparing its manifest version
  /// against what's installed on the device. Drives the row's chip and its
  /// action-button label. `kind` doubles as the chip's CSS class.
  ///   none      — not installed (or package unknown): plain "Install"
  ///   installed — installed but versions can't be compared
  ///   reinstall — same version already installed
  ///   upgrade   — the APK is newer than the installed copy
  ///   downgrade — the APK is older than the installed copy
  function apkStatus(apk: DiscoveredApk): {
    kind: "none" | "installed" | "reinstall" | "upgrade" | "downgrade";
    disabled: boolean;
    label: string;
    button: string;
  } {
    const pkg = apk.package;
    const state = pkg ? apkInstallState[pkg] : undefined;
    if (!pkg || (state !== "enabled" && state !== "disabled")) {
      return { kind: "none", disabled: false, label: "", button: "Install" };
    }
    const disabled = state === "disabled";
    const suffix = disabled ? " (disabled)" : "";
    const dev = apkInstalledVersions[pkg];
    const apkCode = apk.version_code;
    const devCode = dev?.version_code ?? null;

    // Can't compare (either side's versionCode unknown) → flat installed chip.
    if (apkCode == null || devCode == null) {
      return { kind: "installed", disabled, label: `INSTALLED${suffix}`, button: "Reinstall" };
    }

    const from = dev?.version_name;
    const to = apk.version_name;
    const arrow = from && to ? ` ${from} → ${to}` : "";

    if (apkCode > devCode) {
      return { kind: "upgrade", disabled, label: `UPGRADE${arrow}${suffix}`, button: "Upgrade" };
    }
    if (apkCode < devCode) {
      return { kind: "downgrade", disabled, label: `DOWNGRADE${arrow}${suffix}`, button: "Reinstall" };
    }
    const sameVer = to ?? from;
    return {
      kind: "reinstall",
      disabled,
      label: `INSTALLED${sameVer ? ` · v${sameVer}` : ""}${suffix}`,
      button: "Reinstall",
    };
  }

  function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
    return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
  }

  async function loadQuickApps() {
    try {
      quickApps = await api.listQuickApps(serial);
    } catch {
      // Non-fatal — the rest of the tab still works without the catalog.
    }
  }

  async function installQuick(pkg: string) {
    quickBusy = pkg;
    quickMsgPkg = pkg;
    quickMsg = "";
    quickOk = false;
    try {
      const r = await api.installQuickApp(serial, pkg);
      quickOk = r.ok;
      quickMsg = r.ok ? "Installed." : installFailureSummary(r.message);
      if (r.ok) await loadQuickApps();
    } catch (e) {
      quickMsg = String(e);
    } finally {
      quickBusy = null;
    }
  }

  async function setupShizuku() {
    shizukuBusy = true;
    shizukuMsg = "";
    shizukuOk = false;
    try {
      const r = await api.setupShizuku(serial);
      shizukuOk = r.ok;
      shizukuMsg = r.message;
      await loadQuickApps();
      await checkShizuku();
    } catch (e) {
      shizukuMsg = String(e);
    } finally {
      shizukuBusy = false;
    }
  }

  /// Probe whether Shizuku is installed and its service is live — drives the
  /// status dot. Best-effort: a failed probe leaves the dot in the "unknown"
  /// state rather than reporting a false "stopped".
  async function checkShizuku() {
    shizukuCheckBusy = true;
    try {
      shizukuStatus = await api.shizukuStatus(serial);
    } catch {
      shizukuStatus = null;
    } finally {
      shizukuCheckBusy = false;
    }
  }

  onMount(() => {
    const last = localStorage.getItem("shieldopt.lastApkFolder");
    if (last) scanApkFolder(last);
    loadQuickApps();
    checkShizuku();
  });
</script>

<div role="tabpanel" tabindex={0} id="tabpanel-sideload" aria-labelledby="tab-sideload">
<div class="card">
  <div class="card-header">
    <h2>Install APK</h2>
    <div class="header-actions">
      <button onclick={pickApkFolder} disabled={sideloadBusy !== null || discoveryBusy}>
        {discoveryBusy ? "Scanning…" : "Choose folder…"}
      </button>
      <button class="primary" onclick={pickAndInstallApk} disabled={sideloadBusy !== null}>
        {sideloadBusy !== null ? "Installing…" : "Pick file…"}
      </button>
    </div>
  </div>
  <p class="muted small">
    Pick a file directly, or point at a folder and we'll list every APK inside.
    Either way, install runs <code>adb install -r &lt;file&gt;</code>.
  </p>

  {#if discoveredFolder && discoveredApks.length > 0}
    <div class="apk-folder muted small mono">
      {discoveredFolder} — {discoveredApks.length} APK{discoveredApks.length === 1 ? "" : "s"} found
    </div>
    <ul class="apk-list">
      {#each discoveredApks as apk (apk.path)}
        {@const status = apkStatus(apk)}
        <li>
          <div class="apk-row">
            <div class="apk-meta">
              <div class="apk-name">{apk.name}</div>
              <div class="muted small">
                {formatBytes(apk.size_bytes)}
                {#if apk.package}
                  · {apk.package}
                  {#if status.kind !== "none"}
                    <span
                      class="tag"
                      class:installed={status.kind === "installed"}
                      class:reinstall={status.kind === "reinstall"}
                      class:upgrade={status.kind === "upgrade"}
                      class:downgrade={status.kind === "downgrade"}
                      class:is-disabled={status.disabled}>{status.label}</span>
                  {/if}
                {/if}
              </div>
            </div>
            <button
              class="small-action primary"
              onclick={() => installApkPath(apk.path)}
              disabled={sideloadBusy !== null}
            >
              {sideloadBusy === apk.path ? "Installing…" : status.button}
            </button>
          </div>
          {#if sideloadResultPath === apk.path && sideloadResult}
            <div class="install-result" class:ok={sideloadOk} class:bad={!sideloadOk}>
              <span>{sideloadOk ? "✓" : "✕"} {sideloadResult}</span>
              {#if sideloadHint}<span class="muted small"> — {sideloadHint}</span>{/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {:else if discoveredFolder}
    <p class="muted small">No <code>.apk</code> files in {discoveredFolder}.</p>
  {/if}

  {#if sideloadResult && !discoveredApks.some((a) => a.path === sideloadResultPath)}
    <div class="install-result" class:ok={sideloadOk} class:bad={!sideloadOk}>
      <span>{sideloadOk ? "✓" : "✕"} {sideloadResult}</span>
      {#if sideloadHint}<span class="muted small"> — {sideloadHint}</span>{/if}
    </div>
  {/if}
</div>

<div class="card section-card">
  <div class="shizuku-row">
    <div>
      <div class="shizuku-header">
        <h2>Shizuku</h2>
        <span
          class="status-dot"
          class:running={shizukuStatus?.running}
          class:stopped={!!shizukuStatus && shizukuStatus.installed && !shizukuStatus.running}
          class:checking={shizukuCheckBusy}
          aria-hidden="true"
        ></span>
        <span class="shizuku-status muted small">{shizukuStatusLabel}</span>
        <button
          class="icon-check"
          onclick={checkShizuku}
          disabled={shizukuCheckBusy}
          title="Check whether Shizuku's service is running"
          aria-label="Check Shizuku status"
        >
          {shizukuCheckBusy ? "…" : "⟳"}
        </button>
      </div>
      <div class="muted small">
        Installs Shizuku if it's missing, then starts its service so other apps can use
        elevated ADB permissions — no root, and no pairing code (Android TV never shows
        one). The service stops on reboot; press this again to start it back up.
      </div>
      {#if shizukuMsg}
        <div class="install-result" class:ok={shizukuOk} class:bad={!shizukuOk}>
          <span>{shizukuOk ? "✓" : "✕"} {shizukuMsg}</span>
        </div>
      {/if}
    </div>
    <button class="primary" onclick={setupShizuku} disabled={shizukuBusy}>
      {shizukuBusy ? "Starting…" : "Install / Start"}
    </button>
  </div>
</div>

<div class="card section-card">
  <h2>Quick install</h2>
  <p class="muted small">
    We fetch the build that matches your device's CPU from the official source and
    install it. This briefly turns off Play Protect (which flags these apps) and
    restores it afterward.
  </p>
  {#if quickApps.length > 0}
    <ul class="catalog-list">
      {#each quickApps as q (q.package)}
        <li>
          <div>
            <div class="apk-name">{q.name}</div>
            <div class="muted small">{q.description}</div>
            <div class="muted small mono">
              {q.package}
              {#if q.installed}<span class="tag installed">INSTALLED</span>{/if}
            </div>
            {#if quickMsgPkg === q.package && quickMsg}
              <div class="install-result" class:ok={quickOk} class:bad={!quickOk}>
                <span>{quickOk ? "✓" : "✕"} {quickMsg}</span>
              </div>
            {/if}
          </div>
          <button
            class="small-action primary"
            onclick={() => installQuick(q.package)}
            disabled={quickBusy !== null}
          >
            {quickBusy === q.package ? "Installing…" : q.installed ? "Reinstall" : "Install"}
          </button>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="muted small">Catalog not loaded — check the connection and reopen this tab.</p>
  {/if}
</div>
</div>

<style>
  /* Shared scoped utilities duplicated from the page; global rules
     (.muted, button, input) live in the layout and are inherited. */
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
  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .header-actions {
    display: flex;
    gap: 0.8rem;
    align-items: center;
  }
  .small {
    font-size: 0.82rem;
  }
  .mono {
    font-family: ui-monospace, monospace;
  }
  .small-action {
    padding: 0.2rem 0.6rem;
    font-size: 0.78rem;
  }
  .tag {
    font-size: 0.7rem;
    padding: 0.15rem 0.5rem;
    border-radius: 4px;
    letter-spacing: 0.04em;
  }
  .tag.installed,
  .tag.reinstall { background: var(--ok-surface); color: var(--ok); }
  .tag.upgrade { background: var(--accent-glow); color: var(--accent); }
  .tag.downgrade { background: var(--warn-surface-2); color: var(--warn); }
  /* A disabled install is still installed — keep the kind's color but mute it. */
  .tag.is-disabled { opacity: 0.6; }
  code {
    background: var(--bg-inset);
    border: 1px solid var(--border);
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    font-family: ui-monospace, monospace;
    font-size: 0.85em;
  }

  /* Install-APK–specific styles. */
  .apk-folder {
    margin: 0.4rem 0;
    padding: 0.4rem 0.6rem;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: 4px;
    word-break: break-all;
  }
  .apk-list {
    list-style: none;
    padding: 0;
    margin: 0.4rem 0 0.8rem;
  }
  .apk-list li {
    padding: 0.5rem 0;
    border-bottom: 1px solid var(--bg-button);
  }
  .apk-list li:last-child {
    border-bottom: none;
  }
  .apk-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.8rem;
  }
  .install-result {
    margin-top: 0.4rem;
    font-size: 0.88rem;
  }
  .install-result.ok { color: var(--ok); }
  .install-result.bad { color: var(--warn); }
  .apk-name {
    font-family: ui-monospace, monospace;
    font-size: 0.88rem;
    word-break: break-all;
  }
  .section-card {
    margin-top: 1rem;
  }
  .shizuku-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .shizuku-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.3rem;
  }
  .shizuku-header h2 {
    margin: 0;
  }
  .status-dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex-shrink: 0;
    /* Default (not installed / unknown): muted. */
    background: var(--fg-muted);
  }
  .status-dot.stopped {
    background: var(--warn);
  }
  .status-dot.running {
    background: var(--ok);
    box-shadow: 0 0 6px var(--ok);
  }
  .status-dot.checking {
    opacity: 0.5;
  }
  .shizuku-status {
    font-size: 0.8rem;
  }
  .icon-check {
    padding: 0.1rem 0.4rem;
    font-size: 0.85rem;
    line-height: 1;
  }
  .shizuku-row > button {
    white-space: nowrap;
    flex-shrink: 0;
  }
  .catalog-list {
    list-style: none;
    padding: 0;
    margin: 0.5rem 0 0;
  }
  .catalog-list li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.6rem 0;
    border-bottom: 1px solid var(--bg-button);
  }
  .catalog-list li button {
    white-space: nowrap;
    flex-shrink: 0;
  }
</style>
