//! Screen recording — `adb shell screenrecord` spawned as a long-lived child,
//! stopped with SIGINT so the MP4 is finalized (a hard kill truncates it), then
//! pulled to the app's recordings folder and removed from the device.
//!
//! The device path is a fixed constant (no user input to confine), and the
//! spawned child is tracked in `AppState` so a second Start is refused while one
//! is in flight. `screenrecord` caps a single clip at ~3 minutes and records
//! DRM/protected surfaces as black — the UI says so.

use serde::Serialize;
use tauri::State;

use super::apps::ActionResult;
use super::AppState;

/// Fixed on-device capture path — one per device's own `/sdcard`, so no
/// per-session naming is needed and there's no user-supplied path to validate.
const DEVICE_PATH: &str = "/sdcard/_shieldopt_rec.mp4";
const BIT_RATE: &str = "4000000";

#[derive(Serialize)]
pub struct RecordResult {
    pub ok: bool,
    pub message: String,
    /// Absolute local path of the pulled MP4 on success.
    pub path: Option<String>,
}

/// Flatten a serial (which may contain `:` for network devices) into a safe
/// filename stem.
fn filename_safe(serial: &str) -> String {
    serial
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `start_recording` — begin capturing the device screen. Refused if one is
/// already running for this device.
#[tauri::command]
pub async fn start_recording(
    state: State<'_, AppState>,
    serial: String,
) -> Result<ActionResult, String> {
    if state.is_recording(&serial).await {
        return Ok(ActionResult {
            ok: false,
            message: "Already recording on this device.".to_string(),
        });
    }
    let adb = state.adb_snapshot().await;
    let child = adb
        .spawn(&[
            "-s",
            &serial,
            "shell",
            "screenrecord",
            "--bit-rate",
            BIT_RATE,
            DEVICE_PATH,
        ])
        .await
        .map_err(|e| e.to_string())?;
    state.insert_recording(&serial, child).await;
    Ok(ActionResult {
        ok: true,
        message: "Recording… (single clip caps at ~3 min; DRM/protected video records black)."
            .to_string(),
    })
}

/// `stop_recording` — SIGINT the on-device `screenrecord`, wait for it to flush,
/// pull the MP4 into the app's recordings folder, and clean it off the device.
#[tauri::command]
pub async fn stop_recording(
    state: State<'_, AppState>,
    serial: String,
) -> Result<RecordResult, String> {
    stop_recording_impl(state.inner(), &serial).await
}

pub async fn stop_recording_impl(state: &AppState, serial: &str) -> Result<RecordResult, String> {
    let Some(mut child) = state.take_recording(serial).await else {
        return Ok(RecordResult {
            ok: false,
            message: "Not recording on this device.".to_string(),
            path: None,
        });
    };

    let adb = state.adb_snapshot().await;
    // SIGINT lets screenrecord write the MP4 moov atom; a hard kill truncates it.
    let _ = adb.shell(serial, "pkill -INT screenrecord").await;
    // Wait (bounded) for the adb-shell child to exit once screenrecord finishes.
    let _ = tokio::time::timeout(std::time::Duration::from_secs(4), child.wait()).await;

    let dir = state
        .snapshot_dir
        .parent()
        .map(|p| p.join("recordings"))
        .unwrap_or_else(|| state.snapshot_dir.join("recordings"));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("create {}: {e}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    let local = dir.join(format!("{}_{stamp}.mp4", filename_safe(serial)));
    let local_str = local.display().to_string();

    let pull = adb
        .raw_transfer(&["-s", serial, "pull", DEVICE_PATH, &local_str])
        .await;
    // Clean the device file regardless of pull outcome.
    let _ = adb.shell(serial, &format!("rm -f {DEVICE_PATH}")).await;

    match pull {
        Ok(_) if local.exists() => Ok(RecordResult {
            ok: true,
            message: format!("Saved recording to {local_str}"),
            path: Some(local_str),
        }),
        Ok(out) => Ok(RecordResult {
            ok: false,
            message: format!(
                "Recording stopped, but the file didn't transfer: {}",
                out.combined().trim()
            ),
            path: None,
        }),
        Err(e) => Ok(RecordResult {
            ok: false,
            message: format!("Recording stopped, but the pull failed: {e}"),
            path: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::{state_with, MockAdb};

    #[test]
    fn flattens_serial_for_filenames() {
        assert_eq!(filename_safe("192.168.1.5:5555"), "192-168-1-5-5555");
    }

    #[tokio::test]
    async fn stop_when_idle_reports_not_recording() {
        let state = state_with(MockAdb::default());
        let r = stop_recording_impl(&state, "serial").await.unwrap();
        assert!(!r.ok);
        assert!(r.message.contains("Not recording"));
        assert!(r.path.is_none());
    }
}
