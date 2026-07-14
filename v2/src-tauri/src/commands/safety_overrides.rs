//! User-declared safety overrides — packages the user has manually marked
//! "safe / no risk" from the Health page, so a Caution (or curated medium/high
//! risk) badge stops shouting for a package they've decided they understand.
//!
//! Scope + guardrail: an override only *softens* the UI (risk badge + the
//! disable confirm). It can never touch the NEVER_DISABLE hard block — those
//! packages would brick the device, so `set_safety_override` refuses to mark
//! one safe. Storage mirrors `home_tracking`: a best-effort JSON file in
//! `data_dir`; a read/write failure degrades to "no overrides", never to a
//! command error. The set is global (keyed by package, not device) because the
//! safety classification itself is a package-name lookup with no device input.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use tauri::State;
use tracing::warn;

use crate::engine::{is_never_disable, is_valid_package_name};

use super::AppState;

type OverrideSet = BTreeSet<String>;

fn overrides_path(data_dir: &Path) -> PathBuf {
    data_dir.join("safety-overrides.json")
}

async fn load_set(data_dir: &Path) -> OverrideSet {
    let path = overrides_path(data_dir);
    match tokio::fs::read_to_string(&path).await {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            warn!(path = %path.display(), error = %e, "safety-overrides file unreadable; starting fresh");
            OverrideSet::new()
        }),
        Err(_) => OverrideSet::new(), // Usually just "file doesn't exist yet".
    }
}

async fn save_set(data_dir: &Path, set: &OverrideSet) {
    let path = overrides_path(data_dir);
    let _ = tokio::fs::create_dir_all(data_dir).await;
    let text = match serde_json::to_string_pretty(set) {
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

/// `list_safety_overrides` — every package the user has marked safe. The Health
/// page loads this once and layers it over the raw safety classification.
#[tauri::command]
pub async fn list_safety_overrides(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(load_set(&state.data_dir).await.into_iter().collect())
}

/// `set_safety_override` — mark `package` safe (`safe = true`) or clear the
/// override (`safe = false`). Returns the full updated list so the caller can
/// replace its local copy in one assignment.
///
/// Refuses to mark a NEVER_DISABLE package safe: that badge is a bricking-tier
/// guardrail, not a caution the user is entitled to wave off.
#[tauri::command]
pub async fn set_safety_override(
    state: State<'_, AppState>,
    package: String,
    safe: bool,
) -> Result<Vec<String>, String> {
    set_safety_override_impl(&state.data_dir, &package, safe).await
}

async fn set_safety_override_impl(
    data_dir: &Path,
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
    let mut set = load_set(data_dir).await;
    let changed = if safe {
        set.insert(package.to_string())
    } else {
        set.remove(package)
    };
    if changed {
        save_set(data_dir, &set).await;
    }
    Ok(set.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn mark_and_clear_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let list = set_safety_override_impl(dir.path(), "com.google.android.katniss", true)
            .await
            .unwrap();
        assert_eq!(list, vec!["com.google.android.katniss".to_string()]);

        // Idempotent: marking again keeps a single entry.
        let list = set_safety_override_impl(dir.path(), "com.google.android.katniss", true)
            .await
            .unwrap();
        assert_eq!(list, vec!["com.google.android.katniss".to_string()]);

        // Survives a reload from disk.
        assert!(load_set(dir.path())
            .await
            .contains("com.google.android.katniss"));

        // Clearing removes it.
        let list = set_safety_override_impl(dir.path(), "com.google.android.katniss", false)
            .await
            .unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn refuses_to_mark_never_disable_package() {
        let dir = tempfile::tempdir().unwrap();
        let err = set_safety_override_impl(dir.path(), "com.android.systemui", true)
            .await
            .unwrap_err();
        assert!(err.contains("protected"), "got: {err}");
        // Nothing was persisted.
        assert!(load_set(dir.path()).await.is_empty());
    }

    #[tokio::test]
    async fn rejects_invalid_package_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            set_safety_override_impl(dir.path(), "no spaces; reboot", true)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unreadable_file_degrades_to_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_set(dir.path()).await.is_empty());

        tokio::fs::write(overrides_path(dir.path()), "not json")
            .await
            .unwrap();
        assert!(load_set(dir.path()).await.is_empty());
    }
}
