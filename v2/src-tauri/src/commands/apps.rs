//! Per-package action commands — disable / enable / uninstall.
//!
//! Honors the reversibility model (architectural commitment #7): disable is
//! a `pm disable-user --user 0` (reversible via enable), uninstall is a
//! `pm uninstall --user 0` (semi-reversible via `cmd package install-existing`
//! or Play Store).

use std::collections::{HashMap, HashSet};

use serde::Serialize;
use tauri::State;

use crate::adb::{
    parse_disabled_packages_output, parse_installed_packages_output, parse_permission_granted,
    parse_total_pss_by_process, parse_usage_stats, AppUsage,
};
use crate::engine::{classify_safety, is_valid_package_name, Safety};

use super::AppState;

/// Reject malformed package names before they're interpolated into a shell
/// command. Packages from `pm list` are well-formed, but the custom-launcher
/// and any manual-entry path are user-controlled — this keeps a stray value
/// from injecting shell syntax into `adb shell`. Returns an error result to
/// surface verbatim if the name is invalid.
fn reject_invalid_package(package: &str) -> Option<ActionResult> {
    if is_valid_package_name(package) {
        return None;
    }
    Some(ActionResult {
        ok: false,
        message: format!("Refusing to act on invalid package name: {package:?}"),
    })
}

/// `safety_info` — pure lookup the frontend uses to decide between "show a
/// loud confirm", "show a hard block badge", and "no extra ceremony".
/// Cheap; doesn't touch the device.
#[tauri::command]
pub fn safety_info(package: String) -> Safety {
    classify_safety(&package)
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageState {
    Enabled,
    Disabled,
    Missing,
}

/// `package_states` — query the device for the current state of each package
/// in `packages`. Two shell calls in parallel (`pm list packages` and
/// `pm list packages -d`), then categorize.
#[tauri::command]
pub async fn package_states(
    state: State<'_, AppState>,
    serial: String,
    packages: Vec<String>,
) -> Result<HashMap<String, PackageState>, String> {
    let adb = state.adb_snapshot().await;
    let (installed_res, disabled_res) = tokio::join!(
        adb.shell(&serial, "pm list packages"),
        adb.shell(&serial, "pm list packages -d"),
    );
    let installed = installed_res.map_err(|e| format!("pm list packages: {e}"))?;
    let disabled = disabled_res.map_err(|e| format!("pm list packages -d: {e}"))?;

    let installed_set: HashSet<String> = parse_installed_packages_output(&installed.stdout)
        .into_iter()
        .collect();
    let disabled_set: HashSet<String> = parse_disabled_packages_output(&disabled.stdout)
        .into_iter()
        .collect();

    let mut out = HashMap::with_capacity(packages.len());
    for pkg in packages {
        let s = if disabled_set.contains(&pkg) {
            PackageState::Disabled
        } else if installed_set.contains(&pkg) {
            PackageState::Enabled
        } else {
            PackageState::Missing
        };
        out.insert(pkg, s);
    }
    Ok(out)
}

/// The version of an installed package, read from `dumpsys package <pkg>`.
/// `version_code` is what Android compares for upgrade/downgrade; `version_name`
/// is the human string shown in the UI.
#[derive(Serialize)]
pub struct InstalledVersion {
    pub version_code: Option<i64>,
    pub version_name: Option<String>,
}

/// `installed_package_versions` — for each package in `packages`, the version
/// currently installed on the device (absent from the map when not installed).
/// Powers the Install-APK tab's Upgrade / Downgrade / Reinstall labelling: the
/// picked APK's manifest version is compared against what's actually on the box.
///
/// One `dumpsys package <pkg>` per package, run in bounded-concurrency batches
/// so a folder full of installed APKs doesn't spawn dozens of shells at once.
#[tauri::command]
pub async fn installed_package_versions(
    state: State<'_, AppState>,
    serial: String,
    packages: Vec<String>,
) -> Result<HashMap<String, InstalledVersion>, String> {
    installed_package_versions_impl(state.inner(), &serial, &packages).await
}

pub async fn installed_package_versions_impl(
    state: &AppState,
    serial: &str,
    packages: &[String],
) -> Result<HashMap<String, InstalledVersion>, String> {
    /// Simultaneous `dumpsys package` calls to keep in flight.
    const CONCURRENCY: usize = 8;

    let adb = state.adb_snapshot().await;
    // Skip anything that isn't a well-formed package id before it reaches the
    // shell — this list originates from APK manifests, not `pm list`.
    let valid: Vec<&String> = packages
        .iter()
        .filter(|p| is_valid_package_name(p))
        .collect();

    let mut out = HashMap::new();
    for chunk in valid.chunks(CONCURRENCY) {
        let futs = chunk.iter().map(|&pkg| {
            let adb = adb.clone();
            async move {
                let text = adb
                    .shell(serial, &format!("dumpsys package {pkg}"))
                    .await
                    .map(|o| o.stdout)
                    .unwrap_or_default();
                (pkg.clone(), text)
            }
        });
        for (pkg, text) in futures_util::future::join_all(futs).await {
            let version_code = parse_dumpsys_version_code(&text);
            let version_name = parse_dumpsys_version_name(&text);
            // Report only packages that are actually present — an absent package
            // yields no version fields (dumpsys prints "Unable to find …").
            if version_code.is_some() || version_name.is_some() {
                out.insert(
                    pkg,
                    InstalledVersion {
                        version_code,
                        version_name,
                    },
                );
            }
        }
    }
    Ok(out)
}

/// First `versionCode=<n>` in `dumpsys package` output (the primary package
/// block lists it before any secondary users). Ignores the trailing
/// ` minSdk=… targetSdk=…` on the same line.
fn parse_dumpsys_version_code(dumpsys: &str) -> Option<i64> {
    dumpsys.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("versionCode=")?;
        rest.split_whitespace().next()?.parse::<i64>().ok()
    })
}

/// First `versionName=<v>` in `dumpsys package` output.
fn parse_dumpsys_version_name(dumpsys: &str) -> Option<String> {
    dumpsys.lines().find_map(|l| {
        let v = l.trim().strip_prefix("versionName=")?.trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

#[derive(Serialize)]
pub struct OtherPackage {
    pub package: String,
    /// Preinstalled (not in `pm list packages -3`).
    pub system: bool,
    pub enabled: bool,
    /// Friendly name from the curated known-names map, when recognized. Lets the
    /// UI show and search "Everything else" by a real name (e.g. "Artemis")
    /// instead of only the package id. `None` for unrecognized packages.
    pub name: Option<String>,
}

/// Package-name prefixes that belong to the device vendor / OS, not the user.
/// Android flags some of these as third-party (a preinstalled Google language
/// IME updated into `/data` shows up in `pm list packages -3`), which would
/// otherwise mislabel them and bury genuinely-sideloaded apps. Treating these
/// as system keeps the default view focused on what the user actually added.
fn is_first_party_package(pkg: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "com.google.",
        "com.android.",
        "com.nvidia.",
        "com.amazon.",
        "org.chromium.",
    ];
    pkg == "android" || PREFIXES.iter().any(|p| pkg.starts_with(p))
}

/// `list_other_packages` — every installed package that is NOT in the curated
/// catalog, so the App List can act on the long tail (sideloaded apps like
/// SmartTube most of all — they get the same Backup / Copy / Disable tools).
/// Third-party first, then system, names ascending.
#[tauri::command]
pub async fn list_other_packages(
    state: State<'_, AppState>,
    serial: String,
) -> Result<Vec<OtherPackage>, String> {
    list_other_packages_impl(state.inner(), &serial).await
}

pub async fn list_other_packages_impl(
    state: &AppState,
    serial: &str,
) -> Result<Vec<OtherPackage>, String> {
    let adb = state.adb_snapshot().await;
    let (all_res, third_res, disabled_res) = tokio::join!(
        adb.shell(serial, "pm list packages"),
        adb.shell(serial, "pm list packages -3"),
        adb.shell(serial, "pm list packages -d"),
    );
    let all = all_res.map_err(|e| format!("pm list packages: {e}"))?;
    let third = third_res.map_err(|e| format!("pm list packages -3: {e}"))?;
    let disabled = disabled_res.map_err(|e| format!("pm list packages -d: {e}"))?;

    let third: HashSet<String> = parse_installed_packages_output(&third.stdout)
        .into_iter()
        .collect();
    let disabled: HashSet<String> = parse_disabled_packages_output(&disabled.stdout)
        .into_iter()
        .collect();
    let catalog: HashSet<&str> = state
        .app_lists
        .common
        .iter()
        .chain(state.app_lists.shield.iter())
        .chain(state.app_lists.googletv.iter())
        .map(|e| e.package.as_str())
        .collect();

    let mut out: Vec<OtherPackage> = parse_installed_packages_output(&all.stdout)
        .into_iter()
        .filter(|p| !catalog.contains(p.as_str()))
        .map(|package| OtherPackage {
            // System if Android says so OR it's a vendor/OS package Android
            // happens to flag third-party (updated Google IMEs, etc.).
            system: !third.contains(&package) || is_first_party_package(&package),
            enabled: !disabled.contains(&package),
            name: state.known_names.get(&package).cloned(),
            package,
        })
        .collect();
    out.sort_by(|a, b| {
        a.system
            .cmp(&b.system)
            .then_with(|| a.package.cmp(&b.package))
    });
    Ok(out)
}

/// `app_memory_map` — package → resident RAM (MB), from a single `dumpsys
/// meminfo`. Lazy companion to the App List: the UI loads the list first, then
/// fetches this and flags which apps are actually using RAM right now. Most
/// apps return nothing (not running) — the ones that do are the real signal,
/// e.g. an unused video app quietly holding background RAM.
#[tauri::command]
pub async fn app_memory_map(
    state: State<'_, AppState>,
    serial: String,
) -> Result<HashMap<String, f64>, String> {
    app_memory_map_impl(state.inner(), &serial).await
}

pub async fn app_memory_map_impl(
    state: &AppState,
    serial: &str,
) -> Result<HashMap<String, f64>, String> {
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(serial, "dumpsys meminfo")
        .await
        .map_err(|e| format!("dumpsys meminfo: {e}"))?;
    Ok(parse_total_pss_by_process(&out.stdout))
}

/// `app_usage_map` — package → last-used + launch count, from a single
/// `dumpsys usagestats`. Powers the "Review / remove if unused" signal: an app
/// never opened (or not in months) is a strong candidate to disable/uninstall.
#[tauri::command]
pub async fn app_usage_map(
    state: State<'_, AppState>,
    serial: String,
) -> Result<HashMap<String, AppUsage>, String> {
    app_usage_map_impl(state.inner(), &serial).await
}

pub async fn app_usage_map_impl(
    state: &AppState,
    serial: &str,
) -> Result<HashMap<String, AppUsage>, String> {
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(serial, "dumpsys usagestats")
        .await
        .map_err(|e| format!("dumpsys usagestats: {e}"))?;
    Ok(parse_usage_stats(&out.stdout))
}

#[derive(Serialize)]
pub struct ActionResult {
    pub ok: bool,
    /// `pm` stdout/stderr — surfaced to the UI so the user can see the actual
    /// error message when something fails (e.g. "Failure [DELETE_FAILED_…]").
    pub message: String,
}

/// `disable_package` — `pm disable-user --user 0 <pkg>`. Reversible.
///
/// Refuses outright if `package` is on the engine's NEVER_DISABLE list —
/// these would brick the device or break ADB. The user can't override this
/// from the UI; they'd have to use `adb shell` directly.
#[tauri::command]
pub async fn disable_package(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(&package) {
        return Ok(rejection);
    }
    if let Safety::NeverDisable { reason } = classify_safety(&package) {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing to disable {package}: {reason}"),
        });
    }
    run(
        &state,
        &serial,
        &format!("pm disable-user --user 0 {package}"),
    )
    .await
}

/// `enable_package` — `pm enable <pkg>`. Reverses a previous disable.
#[tauri::command]
pub async fn enable_package(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(&package) {
        return Ok(rejection);
    }
    run(&state, &serial, &format!("pm enable {package}")).await
}

/// `trim_caches` — ask the package manager to clear app caches device-wide.
/// The huge byte count means "free everything trimmable"; caches rebuild on
/// next app launch, so no confirmation ceremony is needed.
#[tauri::command]
pub async fn trim_caches(
    state: State<'_, AppState>,
    serial: String,
) -> Result<ActionResult, String> {
    run(&state, &serial, "pm trim-caches 999999999999").await
}

/// `force_stop` — `am force-stop <pkg>`. Kills the app's processes; it
/// restarts on next launch, so unlike disable nothing persists and no safety
/// gate beyond name validation is needed.
#[tauri::command]
pub async fn force_stop(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(&package) {
        return Ok(rejection);
    }
    run(&state, &serial, &format!("am force-stop {package}")).await
}

/// `clear_app_cache` — `pm clear-cache <pkg>`. Drops the app's cached files
/// without touching its data, accounts, or settings; the cache rebuilds on next
/// launch, so (like `trim_caches`) nothing persists and no safety gate beyond
/// name validation is needed.
#[tauri::command]
pub async fn clear_app_cache(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    clear_app_cache_impl(state.inner(), &serial, &package).await
}

pub async fn clear_app_cache_impl(
    state: &AppState,
    serial: &str,
    package: &str,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(package) {
        return Ok(rejection);
    }
    run(state, serial, &format!("pm clear-cache {package}")).await
}

/// `clear_app_data` — `pm clear <pkg>`. Wipes the app's data, cache, accounts,
/// and settings — it resets the app to a fresh-install state. Destructive and
/// not reversible, so the UI gates it behind a loud confirm; here we apply the
/// same NEVER_DISABLE refusal as `disable_package`, since clearing a
/// framework/provider's data can brick the device or sign the user out
/// everywhere.
#[tauri::command]
pub async fn clear_app_data(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    clear_app_data_impl(state.inner(), &serial, &package).await
}

pub async fn clear_app_data_impl(
    state: &AppState,
    serial: &str,
    package: &str,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(package) {
        return Ok(rejection);
    }
    if let Safety::NeverDisable { reason } = classify_safety(package) {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing to clear data for {package}: {reason}"),
        });
    }
    run(state, serial, &format!("pm clear {package}")).await
}

/// `kill_all_background` — `am kill-all`. Asks the activity manager to kill
/// every background process it's willing to (foreground apps survive); they
/// restart on next use, so nothing persists and no safety gate is needed. The
/// device-wide companion to per-app `force_stop`.
#[tauri::command]
pub async fn kill_all_background(
    state: State<'_, AppState>,
    serial: String,
) -> Result<ActionResult, String> {
    run(&state, &serial, "am kill-all").await
}

/// Permission names share the package-name character set; reuse the setting-key
/// allowlist so neither value can carry shell metacharacters into the command.
fn is_valid_permission(permission: &str) -> bool {
    crate::commands::is_valid_setting_key(permission)
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PermissionState {
    Granted,
    Revoked,
    /// Package not installed, or it doesn't declare the permission.
    Missing,
}

/// `app_permission_state` — is `permission` granted to `package` right now?
/// Reads `dumpsys package <pkg>`. Used by the Tweaks "disable the Assistant
/// button" toggle (revoking RECORD_AUDIO from Google's search app).
#[tauri::command]
pub async fn app_permission_state(
    state: State<'_, AppState>,
    serial: String,
    package: String,
    permission: String,
) -> Result<PermissionState, String> {
    app_permission_state_impl(state.inner(), &serial, &package, &permission).await
}

pub async fn app_permission_state_impl(
    state: &AppState,
    serial: &str,
    package: &str,
    permission: &str,
) -> Result<PermissionState, String> {
    if !is_valid_package_name(package) || !is_valid_permission(permission) {
        return Ok(PermissionState::Missing);
    }
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(serial, &format!("dumpsys package {package}"))
        .await
        .map_err(|e| format!("dumpsys package {package}: {e}"))?;
    Ok(match parse_permission_granted(&out.stdout, permission) {
        Some(true) => PermissionState::Granted,
        Some(false) => PermissionState::Revoked,
        None => PermissionState::Missing,
    })
}

/// `set_app_permission` — `pm grant`/`pm revoke <pkg> <permission>`. Reversible.
/// Powers the Assistant-button toggle; the safety gate that matters for disable
/// doesn't apply (revoking one runtime permission can't brick the device), but
/// inputs are still validated so nothing reaches the shell unchecked.
#[tauri::command]
pub async fn set_app_permission(
    state: State<'_, AppState>,
    serial: String,
    package: String,
    permission: String,
    grant: bool,
) -> Result<ActionResult, String> {
    set_app_permission_impl(state.inner(), &serial, &package, &permission, grant).await
}

pub async fn set_app_permission_impl(
    state: &AppState,
    serial: &str,
    package: &str,
    permission: &str,
    grant: bool,
) -> Result<ActionResult, String> {
    if !is_valid_package_name(package) || !is_valid_permission(permission) {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing: invalid package/permission ({package:?}, {permission:?})"),
        });
    }
    let verb = if grant { "grant" } else { "revoke" };
    run(state, serial, &format!("pm {verb} {package} {permission}")).await
}

/// `set_app_op` — `appops set <pkg> <op> allow|deny`. Unlike `pm revoke`,
/// `appops deny` blocks the operation silently without triggering Android's
/// re-grant dialog. Powers the Assistant-button toggle.
#[tauri::command]
pub async fn set_app_op(
    state: State<'_, AppState>,
    serial: String,
    package: String,
    op: String,
    allow: bool,
) -> Result<ActionResult, String> {
    set_app_op_impl(state.inner(), &serial, &package, &op, allow).await
}

pub async fn set_app_op_impl(
    state: &AppState,
    serial: &str,
    package: &str,
    op: &str,
    allow: bool,
) -> Result<ActionResult, String> {
    if !is_valid_package_name(package) {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing: invalid package ({package:?})"),
        });
    }
    if !op.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || op.is_empty() {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing: invalid op ({op:?})"),
        });
    }
    let mode = if allow { "allow" } else { "deny" };
    run(
        state,
        serial,
        &format!("cmd appops set {package} {op} {mode}"),
    )
    .await
}

/// `get_app_op` — reads the current appops mode for `<pkg> <op>`.
/// Returns `"allow"`, `"deny"`, `"ignore"`, `"default"`, or `"missing"`.
#[tauri::command]
pub async fn get_app_op(
    state: State<'_, AppState>,
    serial: String,
    package: String,
    op: String,
) -> Result<String, String> {
    get_app_op_impl(state.inner(), &serial, &package, &op).await
}

pub async fn get_app_op_impl(
    state: &AppState,
    serial: &str,
    package: &str,
    op: &str,
) -> Result<String, String> {
    if !is_valid_package_name(package)
        || !op.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || op.is_empty()
    {
        return Ok("missing".to_string());
    }
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(serial, &format!("cmd appops get {package} {op}"))
        .await
        .map_err(|e| format!("cmd appops get: {e}"))?;
    let stdout = out.stdout.trim().to_lowercase();
    if stdout.contains("allow") {
        Ok("allow".to_string())
    } else if stdout.contains("deny") {
        Ok("deny".to_string())
    } else if stdout.contains("ignore") {
        Ok("ignore".to_string())
    } else if stdout.contains("default") {
        Ok("default".to_string())
    } else {
        Ok("missing".to_string())
    }
}

/// Decode a `pm uninstall` failure into a user-readable hint. Mirrors v1's
/// `Get-UninstallErrorReason` (§16.6). Returns `None` when nothing matches
/// so the caller can fall back to the raw output.
pub fn decode_uninstall_error(stdout: &str) -> Option<&'static str> {
    if stdout.contains("Broken pipe") {
        return Some("Protected system app — cannot be removed. Try Disable instead.");
    }
    if stdout.contains("not installed for") {
        return Some("App not installed for this user.");
    }
    if stdout.contains("DELETE_FAILED_INTERNAL_ERROR") {
        return Some("Internal error — the app may be running. Reboot the device and retry.");
    }
    if stdout.contains("DELETE_FAILED_DEVICE_POLICY_MANAGER") {
        return Some("Blocked by device policy manager (work profile / admin).");
    }
    if stdout.contains("DELETE_FAILED_OWNER_BLOCKED") {
        return Some("Blocked — package is owned by another user or profile.");
    }
    None
}

/// `uninstall_package` — `pm uninstall --user 0 <pkg>`. Semi-reversible via
/// `cmd package install-existing` (if the APK is still on /system) or the
/// Play Store.
///
/// Same NEVER_DISABLE refusal as `disable_package` — uninstalling these has
/// the same brick risk as disabling them, and is less reversible.
#[tauri::command]
pub async fn uninstall_package(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(&package) {
        return Ok(rejection);
    }
    if let Safety::NeverDisable { reason } = classify_safety(&package) {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing to uninstall {package}: {reason}"),
        });
    }
    let mut result = run(&state, &serial, &format!("pm uninstall --user 0 {package}")).await?;
    if !result.ok {
        if let Some(hint) = decode_uninstall_error(&result.message) {
            result.message = format!("{}\n→ {hint}", result.message.trim());
        }
    }
    Ok(result)
}

/// `reinstall_existing` — `cmd package install-existing <pkg>`. Brings back a
/// previously-uninstalled app from /system without a Play Store fetch.
#[tauri::command]
pub async fn reinstall_existing(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    if let Some(rejection) = reject_invalid_package(&package) {
        return Ok(rejection);
    }
    run(
        &state,
        &serial,
        &format!("cmd package install-existing {package}"),
    )
    .await
}

/// `open_play_store` — launch the Play Store detail page for `package` on the
/// device. Use when an app was fully uninstalled and isn't available via
/// `install-existing` (third-party apps, or system apps wiped from /data).
///
/// Reject package strings containing shell metacharacters since the value is
/// interpolated into a URL passed to `am start`. Real package names are
/// `[a-zA-Z0-9_.]` only, so this is more than permissive enough.
#[tauri::command]
pub async fn open_play_store(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    if package.is_empty()
        || package
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '_'))
    {
        return Ok(ActionResult {
            ok: false,
            message: format!("invalid package name: {package}"),
        });
    }
    run(
        &state,
        &serial,
        &format!("am start -a android.intent.action.VIEW -d market://details?id={package}"),
    )
    .await
}

async fn run(state: &AppState, serial: &str, cmd: &str) -> Result<ActionResult, String> {
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(serial, cmd)
        .await
        .map_err(|e| format!("{cmd}: {e}"))?;
    let message = if !out.stdout.trim().is_empty() {
        out.stdout
    } else if !out.stderr.trim().is_empty() {
        out.stderr
    } else {
        "(no output)".to_string()
    };
    // pm's exit codes are unreliable across Android versions — inspect the
    // output for the known failure markers instead.
    let ok = !message.contains("Failure")
        && !message.contains("Error")
        && !message.contains("Exception")
        && !message.contains("not installed for");
    Ok(ActionResult { ok, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_party_packages_classified_as_system() {
        // Vendor/OS packages — system even when Android flags them third-party.
        assert!(is_first_party_package(
            "com.google.android.apps.inputmethod.hindi"
        ));
        assert!(is_first_party_package("com.android.vending"));
        assert!(is_first_party_package("com.nvidia.ota"));
        assert!(is_first_party_package("android"));
        // Genuinely-sideloaded apps stay third-party.
        assert!(!is_first_party_package("com.teamsmart.videomanager.tv"));
        assert!(!is_first_party_package("ca.devmesh.overseerrtv"));
        assert!(!is_first_party_package("air.com.shirogames.evoland12"));
        // Not fooled by a prefix appearing mid-string.
        assert!(!is_first_party_package("org.evil.com.google.fake"));
    }

    #[test]
    fn parses_dumpsys_version_fields() {
        let dump = "  Package [com.enai.launcher] (abc123):\n\
                     versionCode=470002 minSdk=22 targetSdk=30\n\
                     versionName=4.70.2\n";
        assert_eq!(parse_dumpsys_version_code(dump), Some(470002));
        assert_eq!(parse_dumpsys_version_name(dump).as_deref(), Some("4.70.2"));
        assert_eq!(parse_dumpsys_version_code("no version here"), None);
        assert_eq!(parse_dumpsys_version_name("no version here"), None);
    }

    #[tokio::test]
    async fn installed_package_versions_reports_installed_only() {
        use crate::commands::test_support::{state_with, MockAdb};

        let dump = "  Package [com.installed.app] (x):\n\
                     versionCode=12345 minSdk=21 targetSdk=33\n\
                     versionName=1.2.3\n";
        // Key the fixture to the installed id so the absent package falls through
        // to the mock's empty default reply and is omitted from the result.
        let state =
            state_with(MockAdb::default().on_shell("dumpsys package com.installed.app", dump));
        let map = installed_package_versions_impl(
            &state,
            "serial",
            &[
                "com.installed.app".to_string(),
                "com.absent.app".to_string(),
            ],
        )
        .await
        .unwrap();
        let v = map.get("com.installed.app").expect("installed pkg present");
        assert_eq!(v.version_code, Some(12345));
        assert_eq!(v.version_name.as_deref(), Some("1.2.3"));
        assert!(!map.contains_key("com.absent.app"));
    }

    #[test]
    fn decodes_protected_system_app() {
        let out = "Failure [DELETE_FAILED_INTERNAL_ERROR] Broken pipe";
        let hint = decode_uninstall_error(out).expect("should decode");
        assert!(hint.contains("Protected system app"));
    }

    #[test]
    fn decodes_not_installed_for_user() {
        let out = "Failure [not installed for 0]";
        assert!(decode_uninstall_error(out).is_some());
    }

    #[test]
    fn unrecognized_returns_none() {
        assert!(decode_uninstall_error("Something totally weird").is_none());
    }

    #[tokio::test]
    async fn list_other_packages_attaches_known_friendly_names() {
        use crate::commands::test_support::{state_with, MockAdb};
        use std::collections::HashMap;

        // Two sideloads not in the (empty) catalog: one known, one not.
        let mock = MockAdb::default()
            .on_shell(
                "pm list packages -3",
                "package:com.limelight.noir\npackage:com.unknown.app",
            )
            .on_shell(
                "pm list packages",
                "package:com.limelight.noir\npackage:com.unknown.app",
            );
        let mut names = HashMap::new();
        names.insert(
            "com.limelight.noir".to_string(),
            "Artemis (Moonlight)".to_string(),
        );
        let state = state_with(mock).with_known_names(names);

        let others = list_other_packages_impl(&state, "serial").await.unwrap();
        let artemis = others
            .iter()
            .find(|o| o.package == "com.limelight.noir")
            .expect("artemis listed");
        assert_eq!(artemis.name.as_deref(), Some("Artemis (Moonlight)"));
        let unknown = others
            .iter()
            .find(|o| o.package == "com.unknown.app")
            .expect("unknown listed");
        assert_eq!(
            unknown.name, None,
            "unrecognized package has no friendly name"
        );
    }

    #[tokio::test]
    async fn app_memory_map_parses_running_processes() {
        use crate::commands::test_support::{state_with, MockAdb};

        let meminfo = "Total PSS by process:\n\
             243,712K: com.netflix.ninja (pid 2201)\n\
             184,200K: com.teamsmart.videomanager.tv (pid 1899)\n";
        let state = state_with(MockAdb::default().on_shell("dumpsys meminfo", meminfo));
        let map = app_memory_map_impl(&state, "serial").await.unwrap();
        assert!(
            (map.get("com.netflix.ninja").copied().unwrap_or(0.0) - 238.0).abs() < 1.0,
            "netflix ~238 MB, got {:?}",
            map.get("com.netflix.ninja")
        );
        assert!(map.contains_key("com.teamsmart.videomanager.tv"));
        assert!(!map.contains_key("com.not.running"));
    }

    #[tokio::test]
    async fn app_usage_map_reports_last_used() {
        use crate::commands::test_support::{state_with, MockAdb};

        let usage = "package=com.netflix.ninja lastTimeUsed=\"2026-06-01 09:00:00\" appLaunchCount=5\n\
                     package=com.unused.app lastTimeUsed=\"1969-12-31 18:00:00\" appLaunchCount=0\n";
        let state = state_with(MockAdb::default().on_shell("dumpsys usagestats", usage));
        let map = app_usage_map_impl(&state, "serial").await.unwrap();
        assert_eq!(
            map.get("com.netflix.ninja")
                .and_then(|u| u.last_used.as_deref()),
            Some("2026-06-01 09:00:00")
        );
        assert_eq!(
            map.get("com.unused.app").and_then(|u| u.last_used.clone()),
            None
        );
    }

    #[tokio::test]
    async fn app_permission_state_reads_grant() {
        use crate::commands::test_support::{state_with, MockAdb};

        let dump = "android.permission.RECORD_AUDIO: granted=true, flags=[ GRANTED_BY_DEFAULT ]";
        let state = state_with(MockAdb::default().on_shell("dumpsys package", dump));
        let got = app_permission_state_impl(
            &state,
            "serial",
            "com.google.android.katniss",
            "android.permission.RECORD_AUDIO",
        )
        .await
        .unwrap();
        assert_eq!(got, PermissionState::Granted);
    }

    #[tokio::test]
    async fn app_permission_state_missing_when_not_listed() {
        use crate::commands::test_support::{state_with, MockAdb};

        // Empty dumpsys (package absent / no such permission) → Missing.
        let state = state_with(MockAdb::default());
        let got = app_permission_state_impl(
            &state,
            "serial",
            "com.google.android.katniss",
            "android.permission.RECORD_AUDIO",
        )
        .await
        .unwrap();
        assert_eq!(got, PermissionState::Missing);
    }

    #[tokio::test]
    async fn set_app_permission_revoke_succeeds_silently() {
        use crate::commands::test_support::{state_with, MockAdb};

        // pm revoke is silent on success; run() reports ok with "(no output)".
        let state = state_with(MockAdb::default());
        let r = set_app_permission_impl(
            &state,
            "serial",
            "com.google.android.katniss",
            "android.permission.RECORD_AUDIO",
            false,
        )
        .await
        .unwrap();
        assert!(
            r.ok,
            "silent pm revoke should read as success: {}",
            r.message
        );
    }

    #[tokio::test]
    async fn clear_app_data_refuses_never_disable() {
        use crate::commands::test_support::{state_with, MockAdb};

        // Capture the shell log so we can prove no `pm clear` ever reached the
        // device for a bricking-tier package.
        let mock = MockAdb::default();
        let log = mock.shell_log();
        let state = state_with(mock);

        let r = clear_app_data_impl(&state, "serial", "com.android.systemui")
            .await
            .unwrap();
        assert!(
            !r.ok,
            "clearing data for a NEVER_DISABLE package must be refused"
        );
        assert!(
            log.lock().unwrap().is_empty(),
            "no shell command should be sent for a refused clear-data"
        );
    }

    #[tokio::test]
    async fn clear_app_cache_runs_for_normal_package() {
        use crate::commands::test_support::{state_with, MockAdb};

        let mock = MockAdb::default();
        let log = mock.shell_log();
        let state = state_with(mock);

        let r = clear_app_cache_impl(&state, "serial", "com.netflix.ninja")
            .await
            .unwrap();
        assert!(r.ok, "clear-cache on a safe package should succeed");
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|c| c == "pm clear-cache com.netflix.ninja"),
            "the clear-cache command should reach the device"
        );
    }

    #[tokio::test]
    async fn set_app_permission_rejects_injection() {
        use crate::commands::test_support::{state_with, MockAdb};

        let state = state_with(MockAdb::default());
        let r = set_app_permission_impl(
            &state,
            "serial",
            "com.google.android.katniss",
            "RECORD_AUDIO; reboot",
            false,
        )
        .await
        .unwrap();
        assert!(
            !r.ok,
            "a permission with shell metacharacters must be refused"
        );
    }
}
