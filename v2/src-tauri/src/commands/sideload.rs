//! APK sideload — `adb install` against a user-picked file.

use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

use super::AppState;

#[derive(Serialize)]
pub struct InstallApkResult {
    pub ok: bool,
    /// Path that was installed (or attempted).
    pub path: String,
    /// adb's verbatim output — surfaces helpful errors like
    /// `INSTALL_FAILED_VERSION_DOWNGRADE` to the user.
    pub message: String,
    /// Optional decoded hint for common failure codes.
    pub hint: Option<String>,
    /// Package id read from the APK's manifest. Populated only when the
    /// downgrade fallback comes into play — it's the one path where the UI
    /// needs a package id it may not already have (a file picked outside the
    /// scanned folder has no row, and so no decoded manifest).
    pub package: Option<String>,
    /// The install went through with `-d` (allow downgrade) — either because
    /// the caller asked for it, or because a plain attempt tripped the
    /// downgrade guard and we retried.
    pub downgraded: bool,
    /// Android refused the downgrade even with `-d`. It only honors that flag
    /// for debuggable apps (or on a debuggable build), so on a retail device
    /// the only remaining route is uninstall-then-install — which erases the
    /// app's data, so the UI asks before taking it.
    pub downgrade_blocked: bool,
}

/// `install_apk` — `adb -s <serial> install [-r] [-d] <path>`. The frontend
/// uses the dialog plugin to obtain a file path before calling this.
///
/// `allow_downgrade` adds `-d`, which is what lets an older APK replace a
/// newer install. Callers that already know they're downgrading (the folder
/// list compares manifest `versionCode` against the device) pass it up front;
/// everyone else gets one automatic retry when the plain attempt comes back
/// `INSTALL_FAILED_VERSION_DOWNGRADE`.
#[tauri::command]
pub async fn install_apk(
    state: State<'_, AppState>,
    serial: String,
    apk_path: String,
    reinstall: Option<bool>,
    allow_downgrade: Option<bool>,
) -> Result<InstallApkResult, String> {
    let path_buf = PathBuf::from(&apk_path);
    if !path_buf.is_file() {
        return Ok(InstallApkResult {
            ok: false,
            path: apk_path,
            message: "APK file does not exist".to_string(),
            hint: None,
            package: None,
            downgraded: false,
            downgrade_blocked: false,
        });
    }

    let adb = state.adb_snapshot().await;
    let reinstall = reinstall.unwrap_or(true);
    let mut downgrade = allow_downgrade.unwrap_or(false);

    let mut combined = run_install(adb.as_ref(), &serial, &apk_path, reinstall, downgrade).await?;
    let mut ok = combined.contains("Success");

    if !ok && !downgrade && combined.contains("INSTALL_FAILED_VERSION_DOWNGRADE") {
        downgrade = true;
        combined = run_install(adb.as_ref(), &serial, &apk_path, reinstall, true).await?;
        ok = combined.contains("Success");
    }

    let downgrade_blocked = !ok && combined.contains("INSTALL_FAILED_VERSION_DOWNGRADE");
    let package = if downgrade_blocked {
        tokio::task::spawn_blocking(move || read_apk_manifest(&path_buf).package)
            .await
            .ok()
            .flatten()
    } else {
        None
    };
    let hint = decode_install_error(&combined);

    Ok(InstallApkResult {
        ok,
        path: apk_path,
        message: combined,
        hint,
        package,
        downgraded: ok && downgrade,
        downgrade_blocked,
    })
}

/// One `adb install` attempt. Returns adb's output — stdout when it wrote
/// anything, stderr otherwise, since install failures land on either depending
/// on the platform-tools version.
async fn run_install(
    adb: &dyn crate::adb::AdbDriver,
    serial: &str,
    apk_path: &str,
    reinstall: bool,
    allow_downgrade: bool,
) -> Result<String, String> {
    let args = install_args(serial, apk_path, reinstall, allow_downgrade);
    let out = adb
        .raw_transfer(&args)
        .await
        .map_err(|e| format!("adb install: {e}"))?;
    Ok(if out.stdout.trim().is_empty() {
        out.stderr
    } else {
        out.stdout
    })
}

/// Build the `adb install` argv. Split out from `run_install` so the flag
/// combinations stay testable without a device.
fn install_args<'a>(
    serial: &'a str,
    apk_path: &'a str,
    reinstall: bool,
    allow_downgrade: bool,
) -> Vec<&'a str> {
    let mut args = vec!["-s", serial, "install"];
    if reinstall {
        args.push("-r");
    }
    if allow_downgrade {
        args.push("-d");
    }
    args.push(apk_path);
    args
}

#[derive(Serialize)]
pub struct DiscoveredApk {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    /// Package id read from the APK's AndroidManifest.xml, when decodable.
    /// Lets the UI flag APKs that are already installed on the device.
    pub package: Option<String>,
    /// `android:versionCode` from the manifest — the integer Android compares
    /// for upgrade/downgrade decisions. Checked against the installed copy so
    /// the UI can label a row Upgrade / Downgrade / Reinstall instead of a flat
    /// "installed".
    pub version_code: Option<i64>,
    /// `android:versionName` — the human version string (e.g. "4.71.3"), for
    /// display only.
    pub version_name: Option<String>,
}

/// Package id + version fields decoded from an APK's `AndroidManifest.xml`.
struct ApkManifestInfo {
    package: Option<String>,
    version_code: Option<i64>,
    version_name: Option<String>,
}

/// Read `package`, `versionCode`, and `versionName` from an APK's binary
/// `AndroidManifest.xml`. Best-effort: any field that can't be decoded comes
/// back `None`, and a file that isn't a readable APK yields an all-`None`
/// result — the install flow doesn't depend on it.
fn read_apk_manifest(apk_path: &std::path::Path) -> ApkManifestInfo {
    let empty = ApkManifestInfo {
        package: None,
        version_code: None,
        version_name: None,
    };
    let Ok(file) = std::fs::File::open(apk_path) else {
        return empty;
    };
    let Ok(mut zip) = zip::ZipArchive::new(file) else {
        return empty;
    };
    let Ok(mut manifest) = zip.by_name("AndroidManifest.xml") else {
        return empty;
    };
    let mut bytes = Vec::new();
    if std::io::Read::read_to_end(&mut manifest, &mut bytes).is_err() {
        return empty;
    }
    let Ok(doc) = axmldecoder::parse(&bytes) else {
        return empty;
    };
    let Some(axmldecoder::Node::Element(root)) = doc.get_root() else {
        return empty;
    };
    if root.get_tag() != "manifest" {
        return empty;
    }
    let attrs = root.get_attributes();
    // Namespaced attributes decode as `android:versionCode` when the manifest
    // keeps attribute-name strings, or the bare `versionCode` when they've been
    // stripped to resource ids — check both.
    let attr = |name: &str| -> Option<String> {
        attrs
            .get(name)
            .or_else(|| attrs.get(format!("android:{name}").as_str()))
            .cloned()
    };
    ApkManifestInfo {
        package: attr("package"),
        version_code: attr("versionCode").and_then(|v| v.parse::<i64>().ok()),
        version_name: attr("versionName").filter(|v| !v.is_empty()),
    }
}

/// `list_apks_in_folder` — scan `folder` for `.apk` files. Used by the
/// Install APK UI to surface a "pick from these" list without the user
/// re-navigating the file picker. Mirrors v1's auto-discovery of `./apks/`.
///
/// Returns up to 50 entries; deeper recursion intentionally avoided.
#[tauri::command]
pub async fn list_apks_in_folder(folder: String) -> Result<Vec<DiscoveredApk>, String> {
    let dir = PathBuf::from(&folder);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut read = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| format!("read_dir {folder}: {e}"))?;
    while let Some(entry) = read.next_entry().await.transpose() {
        let entry = entry.map_err(|e| format!("read_dir entry: {e}"))?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("apk") {
            continue;
        }
        let metadata = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !metadata.is_file() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let info = read_apk_manifest(&path);
        out.push(DiscoveredApk {
            path: path.display().to_string(),
            name,
            size_bytes: metadata.len(),
            package: info.package,
            version_code: info.version_code,
            version_name: info.version_name,
        });
        if out.len() >= 50 {
            break;
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// Decode the common `INSTALL_FAILED_*` / `DELETE_FAILED_*` codes into a one-line
/// hint. Mirrors v1's `Get-UninstallErrorReason` + the inline decoder in
/// `Install-ApkFile`.
pub(crate) fn decode_install_error(text: &str) -> Option<String> {
    for (needle, hint) in [
        (
            "INSTALL_FAILED_INSUFFICIENT_STORAGE",
            "Not enough free storage on the device — free up space and retry.",
        ),
        (
            "INSTALL_FAILED_VERSION_DOWNGRADE",
            "Installed version is newer than this APK, and Android refused the downgrade even with `-d` (it only honors that for debuggable apps). Uninstall the device's copy first — that erases its data — or use a newer APK.",
        ),
        (
            "INSTALL_FAILED_ALREADY_EXISTS",
            "Same version already installed. Pass `reinstall=true` to force.",
        ),
        (
            "INSTALL_FAILED_OLDER_SDK",
            "APK requires a newer Android version than this device runs.",
        ),
        (
            "INSTALL_FAILED_NO_MATCHING_ABIS",
            "APK doesn't include a native library for this device's CPU architecture.",
        ),
        (
            "INSTALL_FAILED_INVALID_APK",
            "APK file is corrupt or malformed.",
        ),
        (
            "INSTALL_PARSE_FAILED",
            "APK couldn't be parsed (may be corrupt or not actually an APK).",
        ),
    ] {
        if text.contains(needle) {
            return Some(hint.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{decode_install_error, install_args};

    #[test]
    fn install_args_add_downgrade_flag_only_when_asked() {
        assert_eq!(
            install_args("ABC", "/tmp/a.apk", true, false),
            ["-s", "ABC", "install", "-r", "/tmp/a.apk"]
        );
        assert_eq!(
            install_args("ABC", "/tmp/a.apk", true, true),
            ["-s", "ABC", "install", "-r", "-d", "/tmp/a.apk"]
        );
        assert_eq!(
            install_args("ABC", "/tmp/a.apk", false, true),
            ["-s", "ABC", "install", "-d", "/tmp/a.apk"]
        );
    }

    #[test]
    fn decodes_common_install_failures() {
        assert!(decode_install_error("INSTALL_FAILED_INSUFFICIENT_STORAGE")
            .unwrap()
            .contains("storage"));
        assert!(decode_install_error("INSTALL_FAILED_VERSION_DOWNGRADE")
            .unwrap()
            .contains("newer"));
        assert!(decode_install_error("INSTALL_FAILED_NO_MATCHING_ABIS")
            .unwrap()
            .contains("architecture"));
        assert!(decode_install_error("Success").is_none());
    }
}
