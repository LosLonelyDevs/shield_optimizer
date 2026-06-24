//! Display report — read-only enumeration of the panel's modes plus the one
//! settable display tweak's current value.
//!
//! Android TV exposes no ADB way to *set* the HDMI output mode (NVIDIA UI only),
//! so this command is read-only. The one settable item, `match_content_frame_rate`
//! (Settings.Secure), is written by the frontend via the existing `write_setting`
//! command — no setter needed here.

use serde::Serialize;
use tauri::State;

use crate::adb::{
    parse_display_mode, parse_supported_display_modes, DisplayMode, DisplayModeOption,
};

use super::AppState;

#[derive(Serialize)]
pub struct DisplayReport {
    /// The mode currently driving the panel (resolution / refresh / HDR types).
    pub current: DisplayMode,
    /// Every distinct mode the panel advertises, active flagged. Read-only.
    pub supported_modes: Vec<DisplayModeOption>,
    /// `secure.match_content_frame_rate`: "0" never / "1" seamless / "2" always,
    /// or `None` when unset.
    pub match_content_frame_rate: Option<String>,
}

/// `display_report` — one round trip: `dumpsys display` (current + supported
/// modes) plus the current `match_content_frame_rate` value.
#[tauri::command]
pub async fn display_report(
    state: State<'_, AppState>,
    serial: String,
) -> Result<DisplayReport, String> {
    let adb = state.adb_snapshot().await;
    let (display_res, mcfr_res) = tokio::join!(
        adb.shell(&serial, "dumpsys display"),
        adb.shell(&serial, "settings get secure match_content_frame_rate"),
    );
    let display_out = display_res.map_err(|e| format!("dumpsys display: {e}"))?;
    let current = parse_display_mode(&display_out.stdout);
    let supported_modes = parse_supported_display_modes(&display_out.stdout);
    let match_content_frame_rate = mcfr_res.ok().and_then(|o| {
        let v = o.stdout.trim().to_string();
        (!v.is_empty() && v != "null").then_some(v)
    });
    Ok(DisplayReport {
        current,
        supported_modes,
        match_content_frame_rate,
    })
}
