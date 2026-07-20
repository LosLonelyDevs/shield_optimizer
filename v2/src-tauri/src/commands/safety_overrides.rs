//! User-declared safety overrides — packages the user has manually marked
//! "safe / no risk" from the Health page or App List, so a Caution (or curated
//! medium/high risk) badge stops shouting for a package they've decided they
//! understand.
//!
//! Scope + guardrail: an override only *softens* the UI (risk badge + the
//! disable confirm). It can never touch the NEVER_DISABLE hard block — those
//! packages would brick the device, so `set_safety_override` refuses to mark
//! one safe. Storage is a best-effort JSON file in `data_dir`; a read/write
//! failure degrades to "no overrides", never to a command error.
//!
//! Overrides are scoped **per device**, keyed by a stable identity (Android ID,
//! falling back to `ro.serialno`) rather than the adb serial — network serials
//! are `ip:port` and change with DHCP, but the same physical device must keep
//! its marks across reconnects. Files written by the older global scheme (a
//! bare JSON array) are migrated into a `shared` set that still applies to
//! every device, so no pre-existing marks are lost.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::warn;

use crate::adb::AdbDriver;
use crate::engine::{is_never_disable, is_valid_package_name};

use super::AppState;

#[derive(Debug, Default, Serialize, Deserialize)]
struct OverridesFile {
    /// Marks from the pre-per-device format — apply to every device.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    shared: BTreeSet<String>,
    /// Device identity → packages marked safe on that device.
    #[serde(default)]
    devices: BTreeMap<String, BTreeSet<String>>,
}

fn overrides_path(data_dir: &Path) -> PathBuf {
    data_dir.join("safety-overrides.json")
}

async fn load_file(data_dir: &Path) -> OverridesFile {
    let path = overrides_path(data_dir);
    let text = match tokio::fs::read_to_string(&path).await {
        Ok(t) => t,
        Err(_) => return OverridesFile::default(), // Usually "doesn't exist yet".
    };
    if let Ok(file) = serde_json::from_str::<OverridesFile>(&text) {
        return file;
    }
    // Legacy format: a bare array of packages, global across devices.
    match serde_json::from_str::<BTreeSet<String>>(&text) {
        Ok(shared) => OverridesFile {
            shared,
            devices: BTreeMap::new(),
        },
        Err(e) => {
            warn!(path = %path.display(), error = %e, "safety-overrides file unreadable; starting fresh");
            OverridesFile::default()
        }
    }
}

async fn save_file(data_dir: &Path, file: &OverridesFile) {
    let path = overrides_path(data_dir);
    let _ = tokio::fs::create_dir_all(data_dir).await;
    let text = match serde_json::to_string_pretty(file) {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "could not serialize safety overrides");
            return;
        }
    };
    if let Err(e) = tokio::fs::write(&path, text).await {
        warn!(path = %path.display(), error = %e, "could not write safety-overrides file");
    }
}

/// Stable identity for the device behind `serial`. Android ID survives IP
/// changes and adb reconnects (it resets only on factory reset — acceptable:
/// a wiped device has forgotten far more than our marks). Hardware serial is
/// the fallback; the adb serial itself is the last resort so the feature
/// still works offline-ish rather than erroring.
async fn device_key(adb: &Arc<dyn AdbDriver>, serial: &str) -> String {
    if let Ok(o) = adb.shell(serial, "settings get secure android_id").await {
        let id = o.stdout.trim();
        if !id.is_empty() && id != "null" {
            return id.to_string();
        }
    }
    if let Ok(o) = adb.shell(serial, "getprop ro.serialno").await {
        let id = o.stdout.trim();
        if !id.is_empty() {
            return id.to_string();
        }
    }
    serial.to_string()
}

fn effective(file: &OverridesFile, key: &str) -> Vec<String> {
    let mut set = file.shared.clone();
    if let Some(dev) = file.devices.get(key) {
        set.extend(dev.iter().cloned());
    }
    set.into_iter().collect()
}

/// `list_safety_overrides` — every package the user has marked safe on this
/// device (plus legacy global marks). The Health page and App List load this
/// once per device and layer it over the raw safety classification.
#[tauri::command]
pub async fn list_safety_overrides(
    state: State<'_, AppState>,
    serial: String,
) -> Result<Vec<String>, String> {
    let adb = state.adb_snapshot().await;
    let key = device_key(&adb, &serial).await;
    Ok(effective(&load_file(&state.data_dir).await, &key))
}

/// `set_safety_override` — mark `package` safe on this device (`safe = true`)
/// or clear the override (`safe = false`). Returns the device's full updated
/// list so the caller can replace its local copy in one assignment.
///
/// Refuses to mark a NEVER_DISABLE package safe: that badge is a bricking-tier
/// guardrail, not a caution the user is entitled to wave off.
#[tauri::command]
pub async fn set_safety_override(
    state: State<'_, AppState>,
    serial: String,
    package: String,
    safe: bool,
) -> Result<Vec<String>, String> {
    let adb = state.adb_snapshot().await;
    let key = device_key(&adb, &serial).await;
    set_safety_override_impl(&state.data_dir, &key, &package, safe).await
}

async fn set_safety_override_impl(
    data_dir: &Path,
    device: &str,
    package: &str,
    safe: bool,
) -> Result<Vec<String>, String> {
    if !is_valid_package_name(package) {
        return Err(format!("Invalid package name: {package:?}"));
    }
    if safe && is_never_disable(package) {
        return Err(format!(
            "{package} is a protected system package and can't be marked safe — disabling it would brick the device."
        ));
    }
    let mut file = load_file(data_dir).await;
    let changed = if safe {
        file.devices
            .entry(device.to_string())
            .or_default()
            .insert(package.to_string())
    } else {
        // Clearing must actually clear: remove the device mark AND any legacy
        // shared mark, or the badge would spring back from the shared set.
        let from_dev = file
            .devices
            .get_mut(device)
            .map(|s| s.remove(package))
            .unwrap_or(false);
        let from_shared = file.shared.remove(package);
        from_dev || from_shared
    };
    if changed {
        file.devices.retain(|_, set| !set.is_empty());
        save_file(data_dir, &file).await;
    }
    Ok(effective(&file, device))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const DEV_A: &str = "android-id-aaaa";
    const DEV_B: &str = "android-id-bbbb";

    #[tokio::test]
    async fn mark_and_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let list = set_safety_override_impl(dir.path(), DEV_A, "com.google.android.katniss", true)
            .await
            .unwrap();
        assert_eq!(list, vec!["com.google.android.katniss".to_string()]);

        // Idempotent: marking again keeps a single entry.
        let list = set_safety_override_impl(dir.path(), DEV_A, "com.google.android.katniss", true)
            .await
            .unwrap();
        assert_eq!(list, vec!["com.google.android.katniss".to_string()]);

        // Survives a reload from disk.
        let file = load_file(dir.path()).await;
        assert_eq!(
            effective(&file, DEV_A),
            vec!["com.google.android.katniss".to_string()]
        );

        // Clearing removes it.
        let list = set_safety_override_impl(dir.path(), DEV_A, "com.google.android.katniss", false)
            .await
            .unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn overrides_are_scoped_per_device() {
        let dir = tempfile::tempdir().unwrap();
        set_safety_override_impl(dir.path(), DEV_A, "com.example.bloat", true)
            .await
            .unwrap();

        // A different device does not see device A's mark.
        let file = load_file(dir.path()).await;
        assert_eq!(effective(&file, DEV_B), Vec::<String>::new());
        assert_eq!(
            effective(&file, DEV_A),
            vec!["com.example.bloat".to_string()]
        );
    }

    #[tokio::test]
    async fn legacy_global_file_applies_to_every_device_until_cleared() {
        let dir = tempfile::tempdir().unwrap();
        // Old format: bare JSON array.
        tokio::fs::write(
            overrides_path(dir.path()),
            r#"["com.google.android.katniss"]"#,
        )
        .await
        .unwrap();

        let file = load_file(dir.path()).await;
        assert_eq!(
            effective(&file, DEV_A),
            vec!["com.google.android.katniss".to_string()]
        );
        assert_eq!(
            effective(&file, DEV_B),
            vec!["com.google.android.katniss".to_string()]
        );

        // Clearing from one device removes the legacy shared mark for all.
        let list = set_safety_override_impl(dir.path(), DEV_A, "com.google.android.katniss", false)
            .await
            .unwrap();
        assert!(list.is_empty());
        let file = load_file(dir.path()).await;
        assert_eq!(effective(&file, DEV_B), Vec::<String>::new());
    }

    #[tokio::test]
    async fn refuses_to_mark_never_disable_package() {
        let dir = tempfile::tempdir().unwrap();
        let err = set_safety_override_impl(dir.path(), DEV_A, "com.android.systemui", true)
            .await
            .unwrap_err();
        assert!(err.contains("protected"), "got: {err}");
        // Nothing was persisted.
        let file = load_file(dir.path()).await;
        assert!(effective(&file, DEV_A).is_empty());
    }

    #[tokio::test]
    async fn rejects_invalid_package_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            set_safety_override_impl(dir.path(), DEV_A, "no spaces; reboot", true)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unreadable_file_degrades_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(effective(&load_file(dir.path()).await, DEV_A).is_empty());

        tokio::fs::write(overrides_path(dir.path()), "not json")
            .await
            .unwrap();
        assert!(effective(&load_file(dir.path()).await, DEV_A).is_empty());
    }
}
