//! System tab — device settings shortcuts, quick tools, and a small system-info
//! readout. Ports the grab-bag from the Android TV Toolkit's Settings/Tools
//! tabs.
//!
//! Fixed-choice actions (system screens, settings shortcuts) go through a Rust
//! enum allowlist — never a free-form action string from the frontend — so
//! nothing user-controlled is interpolated into `am start`. The one
//! package-shaped input (`launch_app`) is validated the same way as the per-app
//! commands. Plain device-setting writes go through the existing
//! `tuning::write_setting`, so they aren't re-implemented here.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::engine::is_valid_package_name;

use super::apps::ActionResult;
use super::AppState;

/// Run a device shell command and fold both streams into an `ActionResult`,
/// using the same failure heuristic as the per-app actions.
async fn run(state: &AppState, serial: &str, cmd: &str) -> Result<ActionResult, String> {
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell(serial, cmd)
        .await
        .map_err(|e| format!("{cmd}: {e}"))?;
    let combined = out.combined();
    let ok = !combined.contains("Error") && !combined.contains("Exception");
    let message = combined.trim().to_string();
    Ok(ActionResult {
        ok,
        message: if message.is_empty() {
            "ok".to_string()
        } else {
            message
        },
    })
}

/// Allowlisted "open this system screen" targets. The frontend sends the enum
/// name; we map it to a fixed command. No arbitrary action strings ever reach
/// the shell.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemScreen {
    NotificationShade,
    SystemUpdates,
    WifiSettings,
    BluetoothSettings,
    DisplaySettings,
    AppSettings,
    DeveloperOptions,
}

impl SystemScreen {
    fn command(self) -> &'static str {
        match self {
            SystemScreen::NotificationShade => "cmd statusbar expand-notifications",
            SystemScreen::SystemUpdates => "am start -a android.settings.SYSTEM_UPDATE_SETTINGS",
            SystemScreen::WifiSettings => "am start -a android.settings.WIFI_SETTINGS",
            SystemScreen::BluetoothSettings => "am start -a android.settings.BLUETOOTH_SETTINGS",
            SystemScreen::DisplaySettings => "am start -a android.settings.DISPLAY_SETTINGS",
            SystemScreen::AppSettings => "am start -a android.settings.APPLICATION_SETTINGS",
            SystemScreen::DeveloperOptions => {
                "am start -a android.settings.APPLICATION_DEVELOPMENT_SETTINGS"
            }
        }
    }
}

/// `open_system_screen` — open a Settings screen (or pull down the notification
/// shade) on the TV by allowlisted name.
#[tauri::command]
pub async fn open_system_screen(
    state: State<'_, AppState>,
    serial: String,
    screen: SystemScreen,
) -> Result<ActionResult, String> {
    run(&state, &serial, screen.command()).await
}

/// `power_action` — wake or sleep the device. WAKEUP (224) reliably turns the
/// panel on; SLEEP (223) puts it to standby. POWER (26) is deliberately not used
/// here because it toggles (and could wake a device you meant to sleep).
#[tauri::command]
pub async fn power_action(
    state: State<'_, AppState>,
    serial: String,
    wake: bool,
) -> Result<ActionResult, String> {
    let code = if wake { 224 } else { 223 };
    run(&state, &serial, &format!("input keyevent {code}")).await
}

/// `launch_app` — open an app by package via the monkey launcher intent.
#[tauri::command]
pub async fn launch_app(
    state: State<'_, AppState>,
    serial: String,
    package: String,
) -> Result<ActionResult, String> {
    launch_app_impl(state.inner(), &serial, &package).await
}

pub async fn launch_app_impl(
    state: &AppState,
    serial: &str,
    package: &str,
) -> Result<ActionResult, String> {
    if !is_valid_package_name(package) {
        return Ok(ActionResult {
            ok: false,
            message: format!("Refusing to launch invalid package name: {package:?}"),
        });
    }
    run(
        state,
        serial,
        &format!("monkey -p {package} -c android.intent.category.LAUNCHER 1"),
    )
    .await
}

/// `set_play_protect` — turn Google Play Protect (the package verifier) on or
/// off. Off (`enable 0`, `user_consent -1`) lets sideloads install without the
/// "scan with Play Protect?" block; on restores both to `1`. Reversible; the UI
/// surfaces it as a plain toggle.
#[tauri::command]
pub async fn set_play_protect(
    state: State<'_, AppState>,
    serial: String,
    enabled: bool,
) -> Result<ActionResult, String> {
    let cmd = if enabled {
        "settings put global package_verifier_enable 1; \
         settings put global package_verifier_user_consent 1"
    } else {
        "settings put global package_verifier_enable 0; \
         settings put global package_verifier_user_consent -1"
    };
    run(&state, &serial, cmd).await
}

/// `repair_ntp` — reset the time server and nudge a sync. For devices whose
/// clock has drifted, which breaks HTTPS and streaming sign-in.
#[tauri::command]
pub async fn repair_ntp(
    state: State<'_, AppState>,
    serial: String,
) -> Result<ActionResult, String> {
    run(
        &state,
        &serial,
        "settings put global ntp_server pool.ntp.org; \
         am broadcast -a android.intent.action.TIME_TICK",
    )
    .await
}

/// `compile_speed_profile` — `cmd package compile -m speed-profile -a`. Forces
/// the package manager to AOT-compile every app's hot paths from its collected
/// profiles, which can noticeably speed up app launches. It recompiles the whole
/// app set and runs for minutes, so it uses the transfer-sized timeout via
/// `shell_long` (the standard 30s timeout would kill it).
#[tauri::command]
pub async fn compile_speed_profile(
    state: State<'_, AppState>,
    serial: String,
) -> Result<ActionResult, String> {
    let adb = state.adb_snapshot().await;
    let out = adb
        .shell_long(&serial, "cmd package compile -m speed-profile -a")
        .await
        .map_err(|e| format!("cmd package compile: {e}"))?;
    let combined = out.combined();
    // `cmd package compile` prints per-package progress; treat the absence of an
    // error as success rather than dumping hundreds of lines into the UI.
    let ok = !combined.contains("Error") && !combined.contains("Exception");
    Ok(ActionResult {
        ok,
        message: if ok {
            "Speed-profile compile finished — app launches should be snappier.".to_string()
        } else {
            combined.trim().to_string()
        },
    })
}

/// Device facts + the current value of every setting the System tab can change,
/// so the UI can show state and highlight the active choice. The Toolkit's Info
/// tab showed some of these (battery, IME); the rest back the device-settings
/// controls (rotation, screen timeout, GPS, developer options, doze).
#[derive(Serialize)]
pub struct SystemInfo {
    /// Battery charge %, or None on mains-powered boxes that report no battery.
    pub battery_percent: Option<i32>,
    /// Current default input method (IME) component, or None.
    pub input_method: Option<String>,
    /// Play Protect (package verifier) enabled? None when the key is unset.
    pub play_protect_enabled: Option<bool>,
    /// `system user_rotation`: "0"=0°, "1"=90°, "2"=180°, "3"=270°.
    pub user_rotation: Option<String>,
    /// `system screen_off_timeout` in milliseconds.
    pub screen_off_timeout: Option<String>,
    /// `secure location_mode`: "0"=off, "3"=high accuracy.
    pub location_mode: Option<String>,
    /// `global development_settings_enabled`: "1"/"0".
    pub developer_options: Option<String>,
    /// `secure doze_enabled`: "1"/"0".
    pub doze_enabled: Option<String>,
}

/// `system_info` — battery + IME + Play Protect + the System-tab device settings,
/// in two parallel shell calls.
#[tauri::command]
pub async fn system_info(state: State<'_, AppState>, serial: String) -> Result<SystemInfo, String> {
    let adb = state.adb_snapshot().await;
    let (battery_res, settings_res) = tokio::join!(
        adb.shell(&serial, "dumpsys battery"),
        adb.shell(
            &serial,
            "settings get secure default_input_method; \
             settings get global package_verifier_enable; \
             settings get system user_rotation; \
             settings get system screen_off_timeout; \
             settings get secure location_mode; \
             settings get global development_settings_enabled; \
             settings get secure doze_enabled"
        ),
    );
    let battery_percent = battery_res
        .ok()
        .and_then(|o| parse_battery_level(&o.stdout));
    let settings_out = settings_res.map_err(|e| format!("settings get: {e}"))?;
    let mut lines = settings_out.stdout.lines().map(clean);
    let input_method = lines.next().flatten();
    let play_protect_enabled = lines.next().flatten().and_then(|v| match v.as_str() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    });
    let user_rotation = lines.next().flatten();
    let screen_off_timeout = lines.next().flatten();
    let location_mode = lines.next().flatten();
    let developer_options = lines.next().flatten();
    let doze_enabled = lines.next().flatten();
    Ok(SystemInfo {
        battery_percent,
        input_method,
        play_protect_enabled,
        user_rotation,
        screen_off_timeout,
        location_mode,
        developer_options,
        doze_enabled,
    })
}

/// Trim a `settings get` line to a value, mapping empty/`null` to None.
fn clean(s: &str) -> Option<String> {
    let v = s.trim();
    if v.is_empty() || v == "null" {
        None
    } else {
        Some(v.to_string())
    }
}

/// Extract the `level: N` percentage from `dumpsys battery`.
fn parse_battery_level(dumpsys: &str) -> Option<i32> {
    dumpsys.lines().find_map(|l| {
        l.trim()
            .strip_prefix("level:")
            .and_then(|n| n.trim().parse().ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_level_parsed_from_dumpsys() {
        let dump =
            "Current Battery Service state:\n  AC powered: true\n  level: 87\n  scale: 100\n";
        assert_eq!(parse_battery_level(dump), Some(87));
        // Mains-powered box with no battery section.
        assert_eq!(
            parse_battery_level("Current Battery Service state:\n"),
            None
        );
    }

    #[test]
    fn system_screens_map_to_fixed_commands() {
        assert_eq!(
            SystemScreen::WifiSettings.command(),
            "am start -a android.settings.WIFI_SETTINGS"
        );
        assert_eq!(
            SystemScreen::NotificationShade.command(),
            "cmd statusbar expand-notifications"
        );
    }

    #[tokio::test]
    async fn launch_app_refuses_injection() {
        use crate::commands::test_support::{state_with, MockAdb};

        let mock = MockAdb::default();
        let log = mock.shell_log();
        let state = state_with(mock);

        let r = launch_app_impl(&state, "serial", "com.x; reboot")
            .await
            .unwrap();
        assert!(!r.ok, "a package with shell metacharacters must be refused");
        assert!(
            log.lock().unwrap().is_empty(),
            "no shell command should be sent for a refused launch"
        );
    }

    #[tokio::test]
    async fn launch_app_runs_for_valid_package() {
        use crate::commands::test_support::{state_with, MockAdb};

        let mock = MockAdb::default();
        let log = mock.shell_log();
        let state = state_with(mock);

        let r = launch_app_impl(&state, "serial", "com.netflix.ninja")
            .await
            .unwrap();
        assert!(r.ok);
        assert!(log
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == "monkey -p com.netflix.ninja -c android.intent.category.LAUNCHER 1"));
    }
}
