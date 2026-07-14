//! Quick-install — auto-download the right build of a popular app for the
//! device's CPU ABI and sideload it.
//!
//! The catalog lives in `data/app-lists/quick-install.json` (never hard-coded
//! here). For GitHub/GitLab apps we fetch the latest release, hand the asset list
//! to the pure `engine::release_assets::select_asset` to pick the ABI-matched
//! `.apk`, download it with the same reqwest (rustls) client used for
//! platform-tools, and install it. These apps trip Play Protect, so the install
//! is wrapped to disable the verifier and always restore it afterward.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tauri::State;

use crate::adb::{parse_installed_packages_output, AdbDriver};
use crate::engine::{select_asset, ReleaseAsset};

use super::sideload::{decode_install_error, InstallApkResult};
use super::AppState;

/// One catalog entry. Mirrors `quick-install.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuickApp {
    pub name: String,
    pub package: String,
    pub description: String,
    /// `github` | `gitlab` | `url`.
    pub source: String,
    /// `owner/name` for GitHub, `group/project` for GitLab. None for `url`.
    pub repo: Option<String>,
    /// Substring every candidate asset must contain (narrows multi-app releases).
    pub asset_match: Option<String>,
    /// Direct `.apk` URL for `source: "url"`, or a last-resort link otherwise.
    pub fallback_url: Option<String>,
}

#[derive(Serialize)]
pub struct QuickAppRow {
    #[serde(flatten)]
    pub app: QuickApp,
    /// Already present on the device.
    pub installed: bool,
}

/// `list_quick_apps` — the catalog plus each app's installed state.
#[tauri::command]
pub async fn list_quick_apps(
    state: State<'_, AppState>,
    serial: String,
) -> Result<Vec<QuickAppRow>, String> {
    list_quick_apps_impl(state.inner(), &serial).await
}

pub async fn list_quick_apps_impl(
    state: &AppState,
    serial: &str,
) -> Result<Vec<QuickAppRow>, String> {
    let apps = super::loader::load_quick_apps();
    let adb = state.adb_snapshot().await;
    let installed: HashSet<String> = match adb.shell(serial, "pm list packages").await {
        Ok(o) => parse_installed_packages_output(&o.stdout)
            .into_iter()
            .collect(),
        Err(_) => HashSet::new(),
    };
    Ok(apps
        .into_iter()
        .map(|app| QuickAppRow {
            installed: installed.contains(&app.package),
            app,
        })
        .collect())
}

/// A reqwest client with a User-Agent — GitHub's API rejects requests without
/// one. rustls, matching `adb/install.rs`.
pub(crate) fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("shield-optimizer")
        .build()
        .map_err(|e| format!("http client: {e}"))
}

async fn fetch_text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GET {url}: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read {url}: {e}"))
}

pub(crate) async fn fetch_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("GET {url}: {e}"))?
        .error_for_status()
        .map_err(|e| format!("GET {url}: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("download {url}: {e}"))?;
    Ok(bytes.to_vec())
}

// --- Minimal release-API shapes (reqwest has no json feature; parse via serde_json). ---

#[derive(Deserialize)]
struct GithubRelease {
    assets: Vec<GithubAsset>,
}
#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GitlabRelease {
    assets: GitlabAssets,
}
#[derive(Deserialize)]
struct GitlabAssets {
    links: Vec<GitlabLink>,
}
#[derive(Deserialize)]
struct GitlabLink {
    name: String,
    url: String,
    direct_asset_url: Option<String>,
}

/// Resolve the download URL for `app` given the device's ABI list.
pub(crate) async fn resolve_url(
    client: &reqwest::Client,
    app: &QuickApp,
    abis: &[String],
) -> Result<String, String> {
    match app.source.as_str() {
        "url" => app
            .fallback_url
            .clone()
            .ok_or_else(|| format!("{} has no download URL", app.name)),
        "github" => {
            let repo = app
                .repo
                .as_deref()
                .ok_or_else(|| format!("{} is missing a repo", app.name))?;
            let body = fetch_text(
                client,
                &format!("https://api.github.com/repos/{repo}/releases/latest"),
            )
            .await?;
            let release: GithubRelease = serde_json::from_str(&body)
                .map_err(|e| format!("parse GitHub release for {repo}: {e}"))?;
            let assets: Vec<ReleaseAsset> = release
                .assets
                .into_iter()
                .map(|a| ReleaseAsset {
                    name: a.name,
                    url: a.browser_download_url,
                })
                .collect();
            select_url(app, &assets, abis)
        }
        "gitlab" => {
            let repo = app
                .repo
                .as_deref()
                .ok_or_else(|| format!("{} is missing a repo", app.name))?;
            let enc = repo.replace('/', "%2F");
            let body = fetch_text(
                client,
                &format!("https://gitlab.com/api/v4/projects/{enc}/releases?per_page=1"),
            )
            .await?;
            let releases: Vec<GitlabRelease> = serde_json::from_str(&body)
                .map_err(|e| format!("parse GitLab releases for {repo}: {e}"))?;
            let first = releases
                .into_iter()
                .next()
                .ok_or_else(|| format!("no releases for {repo}"))?;
            let assets: Vec<ReleaseAsset> = first
                .assets
                .links
                .into_iter()
                .map(|l| ReleaseAsset {
                    name: l.name,
                    url: l.direct_asset_url.unwrap_or(l.url),
                })
                .collect();
            select_url(app, &assets, abis)
        }
        other => Err(format!("{}: unknown source {other:?}", app.name)),
    }
}

/// Run the pure selector, then fall back to the catalog's `fallback_url`.
fn select_url(app: &QuickApp, assets: &[ReleaseAsset], abis: &[String]) -> Result<String, String> {
    if let Some(found) = select_asset(assets, abis, app.asset_match.as_deref()) {
        return Ok(found.url.clone());
    }
    app.fallback_url.clone().ok_or_else(|| {
        format!(
            "No matching .apk in the latest {} release for this device's CPU ({}).",
            app.name,
            abis.join(", ")
        )
    })
}

/// Install an APK with Play Protect (package verifier) disabled, restoring the
/// prior verifier state afterward whether the install succeeds or fails. These
/// auto-downloaded apps are flagged by Play Protect, which otherwise blocks the
/// install.
pub(crate) async fn install_with_play_protect(
    adb: &std::sync::Arc<dyn AdbDriver>,
    serial: &str,
    apk_path: &str,
) -> (String, bool) {
    // Snapshot current verifier state so we can restore exactly.
    let prior = adb
        .shell(
            serial,
            "settings get global package_verifier_enable; \
             settings get global package_verifier_user_consent",
        )
        .await
        .map(|o| o.stdout)
        .unwrap_or_default();
    let mut lines = prior.lines();
    let prev_enable = lines.next().map(str::trim).unwrap_or("null").to_string();
    let prev_consent = lines.next().map(str::trim).unwrap_or("null").to_string();

    let _ = adb
        .shell(
            serial,
            "settings put global package_verifier_enable 0; \
             settings put global package_verifier_user_consent -1",
        )
        .await;

    let result = adb
        .raw_transfer(&["-s", serial, "install", "-r", "-g", apk_path])
        .await;

    // Restore — `null` means the key was unset, so delete it rather than write "null".
    let restore = |key: &str, val: &str| -> String {
        if val == "null" || val.is_empty() {
            format!("settings delete global {key}")
        } else {
            format!("settings put global {key} {val}")
        }
    };
    let _ = adb
        .shell(
            serial,
            &format!(
                "{}; {}",
                restore("package_verifier_enable", &prev_enable),
                restore("package_verifier_user_consent", &prev_consent)
            ),
        )
        .await;

    match result {
        Ok(out) => {
            let combined = if out.stdout.trim().is_empty() {
                out.stderr
            } else {
                out.stdout
            };
            let ok = combined.contains("Success");
            (combined, ok)
        }
        Err(e) => (format!("adb install: {e}"), false),
    }
}

/// `install_quick_app` — resolve, download, and install the catalog app with
/// `package`.
#[tauri::command]
pub async fn install_quick_app(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<InstallApkResult, String> {
    let app = super::loader::load_quick_apps()
        .into_iter()
        .find(|a| a.package == package)
        .ok_or_else(|| format!("not in the quick-install catalog: {package}"))?;

    let adb = state.adb_snapshot().await;
    let abilist = adb
        .shell(&serial, "getprop ro.product.cpu.abilist")
        .await
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default();
    let abis: Vec<String> = abilist
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let client = http_client()?;
    let url = resolve_url(&client, &app, &abis).await?;
    let bytes = fetch_bytes(&client, &url).await?;

    // Stage to a temp file, install, then clean up regardless of outcome.
    let tmp = std::env::temp_dir().join(format!("shieldopt-quick-{package}.apk"));
    tokio::fs::write(&tmp, &bytes)
        .await
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    let tmp_str = tmp.display().to_string();

    let (message, ok) = install_with_play_protect(&adb, &serial, &tmp_str).await;
    let _ = tokio::fs::remove_file(&tmp).await;

    let hint = decode_install_error(&message);
    Ok(InstallApkResult {
        ok,
        path: format!("{} ({})", app.name, url),
        message,
        hint,
    })
}

const SHIZUKU_PKG: &str = "moe.shizuku.privileged.api";
const SHIZUKU_LEGACY_START: &str = "/sdcard/Android/data/moe.shizuku.privileged.api/start.sh";

/// Start Shizuku's service by exec'ing the native starter (`libshizuku.so`)
/// bundled in its APK — the same binary the app's own start script runs. It
/// only needs an ADB shell (uid 2000), which is exactly what we have, so none
/// of the on-device wireless-debugging pairing flow is required. That matters
/// on Shield: Android TV never exposes the pairing UI, so the in-app "start"
/// path is a dead end there.
///
/// The APK directory name carries a random hash that Android regenerates on
/// every app update, so it has to be resolved at run time via `pm path` rather
/// than stored. `echo NO_STARTER` marks a Shizuku too old to ship the starter,
/// which falls back to the legacy `start.sh`.
const SHIZUKU_START_CMD: &str = concat!(
    "p=$(pm path moe.shizuku.privileged.api | head -n1 | sed 's/^package://; s#/base.apk$##'); ",
    "d=$(ls \"$p/lib\" 2>/dev/null | head -n1); ",
    "if [ -n \"$d\" ] && [ -f \"$p/lib/$d/libshizuku.so\" ]; then \"$p/lib/$d/libshizuku.so\"; ",
    "else echo NO_STARTER; fi"
);

/// True when `shizuku_server` is live on the device — the ground truth for
/// "did it start", rather than trusting the starter's own chatter.
///
/// Polled, because the starter forks the server and exits before the child is
/// in the process table: checking once, immediately, loses that race and reports
/// a healthy start as a failure.
async fn shizuku_running(adb: &std::sync::Arc<dyn AdbDriver>, serial: &str) -> bool {
    for attempt in 0..6 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        if let Ok(o) = adb.shell(serial, "ps -A -o NAME").await {
            if o.stdout.contains("shizuku_server") {
                return true;
            }
        }
    }
    false
}

/// `setup_shizuku` — install Shizuku if it's missing, then start its service.
/// Safe to re-run: an existing install is left alone and only the service is
/// (re)started, which is what makes this usable as a plain "start it again
/// after a reboot" button. Reuses the quick-install downloader + Play-Protect
/// wrap.
#[tauri::command]
pub async fn setup_shizuku(
    state: State<'_, AppState>,
    serial: String,
) -> Result<crate::commands::apps::ActionResult, String> {
    setup_shizuku_impl(state.inner(), &serial).await
}

pub async fn setup_shizuku_impl(
    state: &AppState,
    serial: &str,
) -> Result<crate::commands::apps::ActionResult, String> {
    use crate::commands::apps::ActionResult;

    let adb = state.adb_snapshot().await;

    // 1. Install if not already present.
    let installed = adb
        .shell(serial, &format!("pm list packages {SHIZUKU_PKG}"))
        .await
        .map(|o| o.stdout.contains(SHIZUKU_PKG))
        .unwrap_or(false);
    let freshly_installed = !installed;
    if !installed {
        let app = QuickApp {
            name: "Shizuku".to_string(),
            package: SHIZUKU_PKG.to_string(),
            description: String::new(),
            source: "github".to_string(),
            repo: Some("RikkaApps/Shizuku".to_string()),
            asset_match: None,
            fallback_url: None,
        };
        let abilist = adb
            .shell(serial, "getprop ro.product.cpu.abilist")
            .await
            .map(|o| o.stdout.trim().to_string())
            .unwrap_or_default();
        let abis: Vec<String> = abilist
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let client = http_client()?;
        let url = resolve_url(&client, &app, &abis).await?;
        let bytes = fetch_bytes(&client, &url).await?;
        let tmp = std::env::temp_dir().join("shieldopt-shizuku.apk");
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| format!("write {}: {e}", tmp.display()))?;
        let (msg, ok) = install_with_play_protect(&adb, serial, &tmp.display().to_string()).await;
        let _ = tokio::fs::remove_file(&tmp).await;
        if !ok {
            return Ok(ActionResult {
                ok: false,
                message: format!("Shizuku install failed: {}", msg.trim()),
            });
        }
    }

    // 2. Start the service via the bundled native starter. Restarting an
    // already-running server is fine — the starter kills the old process first.
    let out = adb
        .shell(serial, SHIZUKU_START_CMD)
        .await
        .map_err(|e| format!("start Shizuku: {e}"))?;
    let mut combined = out.combined();

    // 3. Pre-13 Shizuku has no native starter — fall back to the script it
    // writes to external storage on first launch, opening the app to trigger it.
    if combined.contains("NO_STARTER") {
        let _ = adb
            .shell(
                serial,
                &format!("monkey -p {SHIZUKU_PKG} -c android.intent.category.LAUNCHER 1"),
            )
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        combined = adb
            .shell(serial, &format!("sh {SHIZUKU_LEGACY_START}"))
            .await
            .map(|o| o.combined())
            .unwrap_or_default();
    }

    // 4. Trust the process table, not the starter's own output.
    if shizuku_running(&adb, serial).await {
        let prefix = if freshly_installed {
            "Shizuku installed and started"
        } else {
            "Shizuku started"
        };
        return Ok(ActionResult {
            ok: true,
            message: format!(
                "{prefix}. The service runs until the TV reboots — press this again after a \
                 restart to bring it back."
            ),
        });
    }

    Ok(ActionResult {
        ok: false,
        message: format!("Shizuku didn't start: {}", combined.trim()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_parses_and_has_known_apps() {
        let apps = super::super::loader::load_quick_apps();
        assert!(apps.len() >= 4, "quick-install catalog looks thin");
        assert!(apps
            .iter()
            .any(|a| a.package == "com.teamsmart.videomanager.tv"));
        // Every entry needs a usable source.
        for a in &apps {
            match a.source.as_str() {
                "github" | "gitlab" => assert!(a.repo.is_some(), "{} needs a repo", a.name),
                "url" => assert!(a.fallback_url.is_some(), "{} needs a url", a.name),
                other => panic!("{} has unknown source {other:?}", a.name),
            }
        }
    }

    #[test]
    fn url_source_resolves_to_fallback() {
        let app = QuickApp {
            name: "AdGuard".into(),
            package: "com.adguard.android.tv".into(),
            description: String::new(),
            source: "url".into(),
            repo: None,
            asset_match: None,
            fallback_url: Some("https://agrd.io/tvapk".into()),
        };
        let url = select_url(&app, &[], &["arm64-v8a".into()]);
        // url source doesn't go through select_url, but the fallback path must hold.
        assert_eq!(url.unwrap(), "https://agrd.io/tvapk");
    }

    #[tokio::test]
    async fn shizuku_starts_via_native_starter_when_already_installed() {
        use crate::commands::test_support::{state_with, MockAdb};
        let state = state_with(
            MockAdb::default()
                .on_shell("pm list packages", "package:moe.shizuku.privileged.api")
                .on_shell("libshizuku.so", "info: shizuku_server pid is 1403")
                .on_shell("ps -A -o NAME", "system_server\nshizuku_server\n"),
        );
        let r = setup_shizuku_impl(&state, "serial").await.unwrap();
        assert!(r.ok, "should report success: {}", r.message);
        assert!(
            r.message.starts_with("Shizuku started"),
            "an existing install should read as started, not installed: {}",
            r.message
        );
    }

    #[tokio::test]
    async fn shizuku_fails_when_server_is_absent_from_process_table() {
        use crate::commands::test_support::{state_with, MockAdb};
        // Starter claims success, but no `shizuku_server` in `ps` — the process
        // table is the ground truth, so this must not report ok.
        let state = state_with(
            MockAdb::default()
                .on_shell("pm list packages", "package:moe.shizuku.privileged.api")
                .on_shell("libshizuku.so", "info: starter begin")
                .on_shell("ps -A -o NAME", "system_server\n"),
        );
        let r = setup_shizuku_impl(&state, "serial").await.unwrap();
        assert!(!r.ok, "must not claim success without a running server");
    }

    #[tokio::test]
    async fn list_marks_installed() {
        use crate::commands::test_support::{state_with, MockAdb};
        let state = state_with(
            MockAdb::default()
                .on_shell("pm list packages", "package:com.teamsmart.videomanager.tv"),
        );
        let rows = list_quick_apps_impl(&state, "serial").await.unwrap();
        let smarttube = rows
            .iter()
            .find(|r| r.app.package == "com.teamsmart.videomanager.tv")
            .expect("SmartTube in catalog");
        assert!(smarttube.installed, "SmartTube should read as installed");
        let adguard = rows
            .iter()
            .find(|r| r.app.package == "com.adguard.android.tv")
            .expect("AdGuard in catalog");
        assert!(!adguard.installed, "AdGuard should read as not installed");
    }
}
