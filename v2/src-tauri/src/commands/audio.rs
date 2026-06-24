//! Audio surround output — report + safe setters.
//!
//! The two settable keys live in `Settings.Global`; `engine::audio` validates
//! and maps values so the host can't write garbage. Surround changes carry no
//! brick risk but can cause silence on a sink that lacks a format, so the UI
//! warns and these keys are captured by the snapshot system for rollback (see
//! `engine::snapshot::tracked_setting_keys`).

use serde::Serialize;
use tauri::State;

use crate::adb::{parse_active_audio_device, parse_audio_supported_encodings};
use crate::commands::tuning::WriteResult;
use crate::engine::audio::{self, SurroundMode};

use super::{quote_shell_arg, AppState};

/// A manual-mode surround codec toggle.
#[derive(Serialize)]
pub struct AudioFormatRow {
    pub id: u32,
    pub key: &'static str,
    pub label: &'static str,
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct AudioReport {
    /// Parsed surround mode, or `None` when unset/unrecognized.
    pub mode: Option<SurroundMode>,
    /// Raw `encoded_surround_output` value (for display / debugging).
    pub mode_raw: Option<String>,
    /// The manual-mode codec toggles, each flagged enabled if in the CSV.
    pub formats: Vec<AudioFormatRow>,
    /// Best-effort list of encodings the HDMI sink claims to support.
    pub supported_encodings: Vec<String>,
    /// Active output device (HDMI / BUILTIN_SPEAKER / …).
    pub active_device: Option<String>,
}

/// `audio_report` — current surround mode + enabled formats + sink-claimed
/// encodings, in one round trip.
#[tauri::command]
pub async fn audio_report(
    state: State<'_, AppState>,
    serial: String,
) -> Result<AudioReport, String> {
    let adb = state.adb_snapshot().await;
    let (mode_res, fmt_res, audio_res) = tokio::join!(
        adb.shell(&serial, "settings get global encoded_surround_output"),
        adb.shell(
            &serial,
            "settings get global encoded_surround_output_enabled_formats"
        ),
        adb.shell(&serial, "dumpsys audio"),
    );

    let mode_raw = mode_res.ok().and_then(|o| {
        let v = o.stdout.trim().to_string();
        (!v.is_empty() && v != "null").then_some(v)
    });
    let mode = mode_raw
        .as_deref()
        .and_then(SurroundMode::from_setting_value);

    let enabled_ids = fmt_res
        .ok()
        .map(|o| audio::parse_enabled_formats_csv(&o.stdout))
        .unwrap_or_default();
    let formats = audio::SURROUND_FORMATS
        .iter()
        .map(|f| AudioFormatRow {
            id: f.id,
            key: f.key,
            label: f.label,
            enabled: enabled_ids.contains(&f.id),
        })
        .collect();

    let audio_text = audio_res.map(|o| o.stdout).unwrap_or_default();
    let supported_encodings = parse_audio_supported_encodings(&audio_text);
    let active_device = parse_active_audio_device(&audio_text);

    Ok(AudioReport {
        mode,
        mode_raw,
        formats,
        supported_encodings,
        active_device,
    })
}

/// `set_surround_output` — write `encoded_surround_output` and read it back to
/// confirm. Pass `auto` to reset.
#[tauri::command]
pub async fn set_surround_output(
    state: State<'_, AppState>,
    serial: String,
    mode: SurroundMode,
) -> Result<WriteResult, String> {
    let adb = state.adb_snapshot().await;
    let val = mode.to_setting_value();
    adb.shell(
        &serial,
        &format!("settings put global encoded_surround_output {val}"),
    )
    .await
    .map_err(|e| format!("settings put: {e}"))?;
    let got = adb
        .shell(&serial, "settings get global encoded_surround_output")
        .await
        .map_err(|e| format!("settings get: {e}"))?;
    let got = got.stdout.trim();
    if got == val {
        Ok(WriteResult {
            ok: true,
            message: format!("Surround output mode set ({val})."),
        })
    } else {
        Ok(WriteResult {
            ok: false,
            message: format!("Write didn't take — device still reports {got:?}."),
        })
    }
}

/// `set_surround_formats` — write the manual-mode enabled-formats CSV. The
/// engine normalizes/validates the ids, so only known formats reach the device.
#[tauri::command]
pub async fn set_surround_formats(
    state: State<'_, AppState>,
    serial: String,
    format_ids: Vec<u32>,
) -> Result<WriteResult, String> {
    let adb = state.adb_snapshot().await;
    let csv = audio::build_enabled_formats_csv(&format_ids);
    let cmd = format!(
        "settings put global encoded_surround_output_enabled_formats {}",
        quote_shell_arg(&csv)
    );
    adb.shell(&serial, &cmd)
        .await
        .map_err(|e| format!("settings put: {e}"))?;
    Ok(WriteResult {
        ok: true,
        message: if csv.is_empty() {
            "Cleared enabled surround formats.".to_string()
        } else {
            format!("Enabled formats set to {csv}.")
        },
    })
}
