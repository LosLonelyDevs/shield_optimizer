<script lang="ts">
  import { onMount } from "svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { api } from "$lib/api";
  import type { DiscoveredApk, InstalledVersion, QuickAppRow, ShizukuStatus } from "$lib/types";

  let { serial }: { serial: string } = $props();

  type InstallOutcome = {
    ok: boolean;
    message: string;
    hint: string | null;
    /// Set when Android refused the downgrade even with `-d`: the package the
    /// user would have to remove to force the older APK on. null when there's
    /// no package id to act on, in which case no fallback is offered.
    blockedPackage: string | null;
  };

  /// Path of the APK currently installing (null when idle) — per-path so a
  /// multi-APK list only shows the spinner on the row actually installing.
  let sideloadBusy = $state<string | null>(null);
  /// Install outcome per APK path. Keyed rather than single-valued so an
  /// Install-all run leaves every row's result on screen, not just the last.
  let installResults = $state<Record<string, InstallOutcome>>({});
  /// Path of the most recent install, so a file picked outside the currently
  /// listed folder still gets its result rendered (below the list).
  let lastInstalledPath = $state<string | null>(null);
  let scanError = $state<string | null>(null);
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

  // Install-all state: runs the discovered APKs one at a time (adb install is
  // not safely concurrent against a single device) with a stop between files.
  let batchRunning = $state(false);
  let batchCancel = $state(false);
  let batchDone = $state(0);
  let batchTotal = $state(0);
  let batchSummary = $state<string | null>(null);
  let batchOk = $state(false);
  let busy = $derived(sideloadBusy !== null || batchRunning);
  /// The result of a one-off install whose file isn't in the listed folder —
  /// it has no row to render under, so it shows below the list instead.
  let looseResult = $derived(
    lastInstalledPath && !discoveredApks.some((a) => a.path === lastInstalledPath)
      ? (installResults[lastInstalledPath] ?? null)
      : null,
  );

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
    scanError = null;
    try {
      discoveredApks = await api.listApksInFolder(folder);
      discoveredFolder = folder;
      batchSummary = null;
      await refreshInstallState();
    } catch (e) {
      scanError = `Scan failed: ${e}`;
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

  /// Install one APK. `refresh` is turned off by the Install-all loop, which
  /// re-queries once at the end instead of after every file. `allowDowngrade`
  /// adds `-d` up front for rows we already know are downgrades; anything else
  /// gets the backend's automatic retry.
  async function installApkPath(
    path: string,
    refresh = true,
    allowDowngrade = false,
  ): Promise<boolean> {
    sideloadBusy = path;
    lastInstalledPath = path;
    delete installResults[path];
    // A single install invalidates the previous run's tally.
    if (!batchRunning) batchSummary = null;
    try {
      const r = await api.installApk(serial, path, true, allowDowngrade);
      // Friendly summary; the raw adb output is kept for the details line.
      installResults[path] = {
        ok: r.ok,
        message: r.ok ? (r.downgraded ? "Downgraded." : "Installed.") : installFailureSummary(r.message),
        hint: r.hint,
        // The backend only decodes the package when it's needed here.
        blockedPackage: r.downgrade_blocked
          ? (r.package ?? discoveredApks.find((a) => a.path === path)?.package ?? null)
          : null,
      };
      // Refresh so the row's chip flips to the newly-installed version (best
      // effort — a failure here shouldn't clobber the success message).
      if (refresh && r.ok && discoveredFolder) {
        try {
          await refreshInstallState();
        } catch {
          // Keep the "Installed." result even if the re-query fails.
        }
      }
      return r.ok;
    } catch (e) {
      installResults[path] = { ok: false, message: String(e), hint: null, blockedPackage: null };
      return false;
    } finally {
      sideloadBusy = null;
    }
  }

  /// Last resort when Android refuses the downgrade even with `-d` — it only
  /// honors that flag for debuggable apps, so on a retail build the installed
  /// copy has to go first. That takes the app's data with it, hence the ask.
  async function uninstallThenInstall(path: string, pkg: string | null) {
    if (!pkg) return;
    if (
      !confirm(
        `Uninstall ${pkg} from the device, then install this APK?\n\nAndroid won't replace a newer version in place, so the installed copy has to be removed first. Its saved data and sign-in go with it.`,
      )
    )
      return;
    sideloadBusy = path;
    try {
      const r = await api.uninstallPackage(serial, pkg);
      if (!r.ok) {
        installResults[path] = {
          ok: false,
          message: `Uninstall failed: ${r.message}`,
          hint: null,
          blockedPackage: null,
        };
        return;
      }
    } catch (e) {
      installResults[path] = {
        ok: false,
        message: `Uninstall failed: ${e}`,
        hint: null,
        blockedPackage: null,
      };
      return;
    } finally {
      sideloadBusy = null;
    }
    await installApkPath(path);
  }

  /// Install every discovered APK, in list order, one at a time. Each row keeps
  /// its own result; the run-level tally lands in `batchSummary`.
  async function installAll() {
    const targets = discoveredApks.map((a) => a.path);
    if (targets.length === 0 || batchRunning) return;
    batchRunning = true;
    batchCancel = false;
    batchDone = 0;
    batchTotal = targets.length;
    batchSummary = null;
    installResults = {};
    let ok = 0;
    let failed = 0;
    try {
      for (const path of targets) {
        if (batchCancel) break;
        if (await installApkPath(path, false)) ok++;
        else failed++;
        batchDone++;
      }
      if (ok > 0) {
        try {
          await refreshInstallState();
        } catch {
          // Keep the batch results even if the re-query fails.
        }
      }
      // Stopping early isn't a failure — only real install errors mark it bad.
      const skipped = batchTotal - batchDone;
      batchOk = failed === 0;
      batchSummary =
        `Installed ${ok} of ${batchTotal}` +
        (failed > 0 ? ` · ${failed} failed` : "") +
        (skipped > 0 ? ` · ${skipped} skipped` : "") +
        ".";
    } finally {
      batchRunning = false;
      batchCancel = false;
    }
  }

  /// Turn raw `adb install` failure output into a one-line summary. The full
  /// text still shows in the details line; this is the headline.
  function installFailureSummary(raw: string): string {
    const m = raw.match(/INSTALL_FAILED_[A-Z_]+|INSTALL_PARSE_FAILED[A-Z_]*/);
    if (m) {
      if (m[0].includes("ALREADY_EXISTS")) return "Already installed (same version).";
      if (m[0].includes("VERSION_DOWNGRADE")) return "Downgrade refused — a newer version is installed.";
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
  ///   reinstall — same versionCode already installed
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
      return { kind: "downgrade", disabled, label: `DOWNGRADE${arrow}${suffix}`, button: "Downgrade" };
    }
    // Equal versionCode. The chip describes the device, so the version it names
    // is always the device's — never the APK's. A build that bumps versionName
    // without bumping versionCode lands here with the two names differing, so
    // name both rather than passing the APK's off as what's installed.
    const devVer = from ? ` · v${from}` : "";
    const apkVer = to && to !== from ? ` · APK v${to}` : "";
    return {
      kind: "reinstall",
      disabled,
      label: `INSTALLED${devVer}${apkVer}${suffix}`,
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
      <button onclick={pickApkFolder} disabled={busy || discoveryBusy}>
        {discoveryBusy ? "Scanning…" : "Choose folder…"}
      </button>
      <button class="primary" onclick={pickAndInstallApk} disabled={busy}>
        {sideloadBusy !== null && !batchRunning ? "Installing…" : "Pick file…"}
      </button>
    </div>
  </div>
  <p class="muted small">
    Pick a file directly, or point at a folder and we'll list every APK inside.
    Either way, install runs <code>adb install -r &lt;file&gt;</code> — and
    <code>-d</code> on top when the APK is older than what's on the device, so
    downgrades go through where Android allows them.
  </p>

  {#if discoveredFolder && discoveredApks.length > 0}
    <div class="apk-folder-row">
      <div class="apk-folder muted small mono">
        {discoveredFolder} — {discoveredApks.length} APK{discoveredApks.length === 1 ? "" : "s"} found
      </div>
      {#if discoveredApks.length > 1}
        <div class="batch-actions">
          <button class="small-action primary" onclick={installAll} disabled={busy || discoveryBusy}>
            {batchRunning
              ? `Installing ${Math.min(batchDone + 1, batchTotal)}/${batchTotal}…`
              : `Install all (${discoveredApks.length})`}
          </button>
          {#if batchRunning}
            <button class="small-action" onclick={() => (batchCancel = true)} disabled={batchCancel}>
              {batchCancel ? "Stopping…" : "Stop"}
            </button>
          {/if}
        </div>
      {/if}
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
              onclick={() => installApkPath(apk.path, true, status.kind === "downgrade")}
              disabled={busy}
            >
              {sideloadBusy === apk.path
                ? status.kind === "downgrade"
                  ? "Downgrading…"
                  : "Installing…"
                : status.button}
            </button>
          </div>
          {#if installResults[apk.path]}
            {@render resultLine(apk.path, installResults[apk.path])}
          {/if}
        </li>
      {/each}
    </ul>
    {#if batchSummary}
      <div class="install-result" class:ok={batchOk} class:bad={!batchOk}>
        <span>{batchOk ? "✓" : "✕"} {batchSummary}</span>
      </div>
    {/if}
  {:else if discoveredFolder}
    <p class="muted small">No <code>.apk</code> files in {discoveredFolder}.</p>
  {/if}

  {#if scanError}
    <div class="install-result bad"><span>✕ {scanError}</span></div>
  {/if}

  {#if looseResult && lastInstalledPath}
    {@render resultLine(lastInstalledPath, looseResult)}
  {/if}
</div>

{#snippet resultLine(path: string, res: InstallOutcome)}
  <div class="install-result" class:ok={res.ok} class:bad={!res.ok}>
    <span>{res.ok ? "✓" : "✕"} {res.message}</span>
    {#if res.hint}<span class="muted small"> — {res.hint}</span>{/if}
    {#if res.blockedPackage}
      <button
        class="small-action"
        onclick={() => uninstallThenInstall(path, res.blockedPackage)}
        disabled={busy}
      >
        Uninstall &amp; install
      </button>
    {/if}
  </div>
{/snippet}

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
  .apk-folder-row {
    display: flex;
    align-items: center;
    gap: 0.8rem;
  }
  .apk-folder {
    flex: 1;
    min-width: 0;
    margin: 0.4rem 0;
    padding: 0.4rem 0.6rem;
    background: var(--bg-inset);
    border: 1px solid var(--border);
    border-radius: 4px;
    word-break: break-all;
  }
  .batch-actions {
    display: flex;
    gap: 0.4rem;
    flex-shrink: 0;
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
  .install-result button {
    margin-left: 0.6rem;
    vertical-align: baseline;
  }
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
