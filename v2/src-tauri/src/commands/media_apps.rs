//! Media-app integration — detection, Kodi config staging, and the
//! `WRITE_SECURE_SETTINGS` grant that per-app refresh-rate apps need.
//!
//! No app's private storage is touched: Kodi config is staged to /sdcard for the
//! user to import (Android 11 scoped storage blocks writing Kodi's app dir
//! directly), Plex is detect-only (its prefs are root-only), and the refresh-rate
//! app is detected/granted — we can't write its per-app profiles (root-only), so
//! the UI guides the user through them.

use serde::Serialize;
use tauri::State;

use crate::adb::parse_installed_packages_output;
use crate::engine::kodi::{self, KodiProfile};

use super::apps::ActionResult;
use super::{quote_shell_arg, AppState};

const KODI_PKG: &str = "org.xbmc.kodi";
const PLEX_PKG: &str = "com.plexapp.android";

/// Where the generated Kodi config is staged. /sdcard root stays reachable by
/// adb under Android 11 scoped storage (unlike `/sdcard/Android/data/...`).
const KODI_CONFIG_STAGE_PATH: &str = "/sdcard/advancedsettings.xml";

#[derive(Serialize)]
pub struct MediaApps {
    pub kodi: bool,
    pub plex: bool,
    /// The installed per-app refresh-rate app's package id, if one is present.
    pub refresh_rate_app: Option<String>,
}

/// `detect_media_apps` — one `pm list packages` scan → which media apps are
/// present. The refresh-rate app's exact id isn't fixed across builds, so we
/// match any installed package whose id contains "refreshrate".
#[tauri::command]
pub async fn detect_media_apps(
    state: State<'_, AppState>,
    serial: String,
) -> Result<MediaApps, String> {
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(&serial, "pm list packages")
        .await
        .map_err(|e| format!("pm list packages: {e}"))?;
    let installed = parse_installed_packages_output(&out.stdout);
    let refresh_rate_app = installed
        .iter()
        .find(|p| {
            let low = p.to_ascii_lowercase().replace(['_', '.'], "");
            low.contains("refreshrate")
        })
        .cloned();
    Ok(MediaApps {
        kodi: installed.iter().any(|p| p == KODI_PKG),
        plex: installed.iter().any(|p| p == PLEX_PKG),
        refresh_rate_app,
    })
}

#[derive(Serialize)]
pub struct StageResult {
    pub ok: bool,
    pub message: String,
    /// Device path the config was written to (for the import instructions).
    pub device_path: String,
}

/// `stage_kodi_config` — render `advancedsettings.xml` from `profile` and write
/// it to /sdcard for the user to import into Kodi's userdata. We don't write
/// Kodi's app dir directly (scoped storage); the UI shows the import steps.
#[tauri::command]
pub async fn stage_kodi_config(
    state: State<'_, AppState>,
    serial: String,
    profile: KodiProfile,
) -> Result<StageResult, String> {
    let adb = state.adb_snapshot().await;
    let xml = kodi::render_advancedsettings(&profile);
    // `printf '%s'` preserves the newlines; single-quoting neutralizes the XML
    // so nothing in it is re-parsed by the device shell.
    let cmd = format!(
        "printf '%s' {} > {}",
        quote_shell_arg(&xml),
        quote_shell_arg(KODI_CONFIG_STAGE_PATH)
    );
    let out = adb
        .shell(&serial, &cmd)
        .await
        .map_err(|e| format!("stage config: {e}"))?;
    if out.shell_reported_failure() {
        return Ok(StageResult {
            ok: false,
            message: out.combined(),
            device_path: KODI_CONFIG_STAGE_PATH.to_string(),
        });
    }
    Ok(StageResult {
        ok: true,
        message: format!("Wrote advancedsettings.xml to {KODI_CONFIG_STAGE_PATH}."),
        device_path: KODI_CONFIG_STAGE_PATH.to_string(),
    })
}

/// `grant_write_secure_settings` — grant the permission the per-app refresh-rate
/// apps need (adb grants it without root). Reuses the apps permission path,
/// which validates the package name.
#[tauri::command]
pub async fn grant_write_secure_settings(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    super::apps::set_app_permission_impl(
        state.inner(),
        &serial,
        &package,
        "android.permission.WRITE_SECURE_SETTINGS",
        true,
    )
    .await
}
