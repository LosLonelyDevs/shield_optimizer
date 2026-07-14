//! Boot-persistent tweaks — settings the user wants held across a reboot.
//!
//! Some settings don't survive a restart. `background_process_limit` is the
//! motivating one: Android drops it back to Standard on boot. The device can't
//! fix this itself — there's no cron on Android, the ADB user isn't root so it
//! can't install a boot script, and the one loophole that would let an on-device
//! app do it (Shizuku self-starting via wireless debugging) doesn't exist on
//! Shield, where NVIDIA ships no wireless-debugging UI at all. So re-applying
//! has to come from the host, which means here.
//!
//! The user opts a setting in, and we remember the value they chose. On the
//! next connect, `reapply_boot_tweaks` compares what the device currently
//! reports against what they asked for and re-writes only the ones that drifted.
//! Comparing values rather than tracking uptime means we don't have to detect a
//! reboot at all — a reboot just shows up as drift, and so does anything else
//! that resets the setting behind our back.
//!
//! Storage mirrors `safety_overrides`: a best-effort JSON file in `data_dir`
//! where a read/write failure degrades to "nothing remembered", never to a
//! command error. Keyed by serial, because the chosen value is per device.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;
use tracing::warn;

use super::tuning::build_setting_command;
use super::AppState;

/// One remembered setting. `namespace` is global/secure/system, matching the
/// `settings` command's own vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct BootTweak {
    pub namespace: String,
    pub key: String,
    pub value: String,
}

/// serial -> "namespace/key" -> value.
type Remembered = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Serialize)]
pub struct ReapplyResult {
    /// Keys that had drifted and were re-written — empty on the common path
    /// where nothing changed since last time.
    pub reapplied: Vec<String>,
    pub ok: bool,
    pub message: String,
}

fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("boot-tweaks.json")
}

async fn load_all(data_dir: &Path) -> Remembered {
    let path = store_path(data_dir);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            warn!(path = %path.display(), error = %e, "boot-tweaks file unreadable; starting fresh");
            Remembered::new()
        }),
        Err(_) => Remembered::new(), // Usually just "file doesn't exist yet".
    }
}

async fn save_all(data_dir: &Path, all: &Remembered) {
    let path = store_path(data_dir);
    let _ = tokio::fs::create_dir_all(data_dir).await;
    let text = match serde_json::to_string_pretty(all) {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "could not serialize boot tweaks");
            return;
        }
    };
    if let Err(e) = tokio::fs::write(&path, text).await {
        warn!(path = %path.display(), error = %e, "could not write boot-tweaks file");
    }
}

fn entry_key(namespace: &str, key: &str) -> String {
    format!("{namespace}/{key}")
}

fn split_entry_key(entry: &str) -> Option<(&str, &str)> {
    entry.split_once('/')
}

/// `list_boot_tweaks` — everything remembered for `serial`, so the UI can show
/// which settings are pinned.
#[tauri::command]
pub async fn list_boot_tweaks(
    state: State<'_, AppState>,
    serial: String,
) -> Result<Vec<BootTweak>, String> {
    Ok(list_boot_tweaks_impl(&state.data_dir, &serial).await)
}

async fn list_boot_tweaks_impl(data_dir: &Path, serial: &str) -> Vec<BootTweak> {
    load_all(data_dir)
        .await
        .get(serial)
        .map(|m| {
            m.iter()
                .filter_map(|(entry, value)| {
                    let (namespace, key) = split_entry_key(entry)?;
                    Some(BootTweak {
                        namespace: namespace.to_string(),
                        key: key.to_string(),
                        value: value.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `set_boot_tweak` — pin `key` at `value` for `serial`, or forget it when
/// `value` is empty. Empty means "back to the platform default", and the
/// platform already gives us that on boot, so there'd be nothing to re-apply.
#[tauri::command]
pub async fn set_boot_tweak(
    state: State<'_, AppState>,
    serial: String,
    namespace: String,
    key: String,
    value: String,
) -> Result<Vec<BootTweak>, String> {
    set_boot_tweak_impl(&state.data_dir, &serial, &namespace, &key, &value).await
}

async fn set_boot_tweak_impl(
    data_dir: &Path,
    serial: &str,
    namespace: &str,
    key: &str,
    value: &str,
) -> Result<Vec<BootTweak>, String> {
    // Validate through the same builder the live write path uses, so a value we
    // pin is one we can actually replay later.
    build_setting_command(namespace, key, value)?;

    let mut all = load_all(data_dir).await;
    let per_device = all.entry(serial.to_string()).or_default();
    if value.is_empty() {
        per_device.remove(&entry_key(namespace, key));
    } else {
        per_device.insert(entry_key(namespace, key), value.to_string());
    }
    if per_device.is_empty() {
        all.remove(serial);
    }
    save_all(data_dir, &all).await;
    Ok(list_boot_tweaks_impl(data_dir, serial).await)
}

/// `reapply_boot_tweaks` — re-write any pinned setting whose on-device value has
/// drifted from what the user chose. Called on connect; a no-op when nothing
/// drifted, so it's cheap to run every time.
#[tauri::command]
pub async fn reapply_boot_tweaks(
    state: State<'_, AppState>,
    serial: String,
) -> Result<ReapplyResult, String> {
    reapply_boot_tweaks_impl(state.inner(), &serial).await
}

async fn reapply_boot_tweaks_impl(state: &AppState, serial: &str) -> Result<ReapplyResult, String> {
    let adb = state.adb_snapshot().await;
    let wanted = list_boot_tweaks_impl(&state.data_dir, serial).await;
    if wanted.is_empty() {
        return Ok(ReapplyResult {
            reapplied: Vec::new(),
            ok: true,
            message: String::new(),
        });
    }

    let mut reapplied = Vec::new();
    let mut failures = Vec::new();
    for tweak in &wanted {
        let get = format!("settings get {} {}", tweak.namespace, tweak.key);
        let current = match adb.shell(serial, &get).await {
            Ok(o) => o.stdout.trim().to_string(),
            Err(e) => {
                failures.push(format!("{}: {e}", tweak.key));
                continue;
            }
        };
        if current == tweak.value {
            continue; // Still as the user left it.
        }
        let cmd = build_setting_command(&tweak.namespace, &tweak.key, &tweak.value)?;
        match adb.shell(serial, &cmd).await {
            Ok(_) => reapplied.push(tweak.key.clone()),
            Err(e) => failures.push(format!("{}: {e}", tweak.key)),
        }
    }

    let ok = failures.is_empty();
    let message = if !ok {
        format!("Could not re-apply {}", failures.join("; "))
    } else if reapplied.is_empty() {
        String::new()
    } else {
        format!(
            "Re-applied after reboot: {}. Android resets these on restart.",
            reapplied.join(", ")
        )
    };
    Ok(ReapplyResult {
        reapplied,
        ok,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::MockAdb;
    use crate::commands::AppState;
    use crate::engine::AppListBundle;
    use std::sync::Arc;

    fn state_at(dir: &Path, mock: MockAdb) -> AppState {
        AppState::new(Arc::new(mock), AppListBundle::default(), dir.to_path_buf())
    }

    #[tokio::test]
    async fn pin_then_forget_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let pinned =
            set_boot_tweak_impl(dir.path(), "S1", "global", "background_process_limit", "2")
                .await
                .unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].value, "2");

        // Empty value = "back to Standard" = nothing worth replaying.
        let cleared =
            set_boot_tweak_impl(dir.path(), "S1", "global", "background_process_limit", "")
                .await
                .unwrap();
        assert!(cleared.is_empty(), "empty value should forget the pin");
    }

    #[tokio::test]
    async fn pins_are_per_device() {
        let dir = tempfile::tempdir().unwrap();
        set_boot_tweak_impl(dir.path(), "S1", "global", "background_process_limit", "2")
            .await
            .unwrap();
        set_boot_tweak_impl(dir.path(), "S2", "global", "background_process_limit", "4")
            .await
            .unwrap();
        assert_eq!(
            list_boot_tweaks_impl(dir.path(), "S1").await[0].value,
            "2",
            "one device's pin must not overwrite another's"
        );
        assert_eq!(list_boot_tweaks_impl(dir.path(), "S2").await[0].value, "4");
    }

    #[tokio::test]
    async fn rewrites_only_the_drifted_setting() {
        let dir = tempfile::tempdir().unwrap();
        set_boot_tweak_impl(dir.path(), "S1", "global", "background_process_limit", "2")
            .await
            .unwrap();
        set_boot_tweak_impl(dir.path(), "S1", "global", "window_animation_scale", "0.5")
            .await
            .unwrap();

        // The limit came back as Standard (`null`) after a reboot; the animation
        // scale survived.
        let mock = MockAdb::default()
            .on_shell("settings get global background_process_limit", "null")
            .on_shell("settings get global window_animation_scale", "0.5");
        let state = state_at(dir.path(), mock);

        let r = reapply_boot_tweaks_impl(&state, "S1").await.unwrap();
        assert!(r.ok);
        assert_eq!(
            r.reapplied,
            vec!["background_process_limit"],
            "only the setting that drifted should be re-written"
        );
    }

    #[tokio::test]
    async fn no_pins_means_no_adb_traffic() {
        let dir = tempfile::tempdir().unwrap();
        let state = state_at(dir.path(), MockAdb::default());
        let r = reapply_boot_tweaks_impl(&state, "S1").await.unwrap();
        assert!(r.ok);
        assert!(r.reapplied.is_empty());
        assert!(r.message.is_empty(), "silent when there's nothing pinned");
    }
}
