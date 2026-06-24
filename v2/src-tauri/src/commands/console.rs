//! Raw ADB shell console — the one place arbitrary shell is allowed.
//!
//! This is the intentional escape hatch: a power user can run any
//! `adb -s <serial> shell <command>` and see the combined output. It
//! deliberately bypasses the curated catalog and the safety classifier (those
//! exist so the *guided* surfaces can't brick a device; this surface is the
//! manual override the user explicitly opts into). It is still device-scoped —
//! always routed through `shell`, never `raw` — so it can't reach `adb`
//! subcommands like `disconnect` or `kill-server`, and a length cap keeps a
//! runaway paste from being sent.

use serde::Serialize;
use tauri::State;

use super::AppState;

const MAX_CMD_LEN: usize = 2000;

#[derive(Serialize)]
pub struct ConsoleResult {
    /// True when the adb invocation itself succeeded (exit 0). On-device tools
    /// like `pm` can still print "Failure" with exit 0 — the raw output shows
    /// that; this flag is only about reaching the device.
    pub ok: bool,
    /// Combined stdout + stderr, trimmed. On a nonzero exit, the adb error text.
    pub output: String,
}

/// `run_shell` — execute one `adb shell` command against `serial`.
#[tauri::command]
pub async fn run_shell(
    state: State<'_, AppState>,
    serial: String,
    command: String,
) -> Result<ConsoleResult, String> {
    run_shell_impl(state.inner(), &serial, &command).await
}

pub async fn run_shell_impl(
    state: &AppState,
    serial: &str,
    command: &str,
) -> Result<ConsoleResult, String> {
    let command = command.trim();
    if command.is_empty() {
        return Ok(ConsoleResult {
            ok: false,
            output: "(empty command)".to_string(),
        });
    }
    if command.len() > MAX_CMD_LEN {
        return Ok(ConsoleResult {
            ok: false,
            output: format!(
                "Command too long ({} > {MAX_CMD_LEN} chars).",
                command.len()
            ),
        });
    }
    // Be forgiving of a pasted "adb shell …" prefix — we already run via shell.
    let command = command.strip_prefix("adb shell ").unwrap_or(command).trim();

    let adb = state.adb_snapshot().await;
    match adb.shell(serial, command).await {
        Ok(out) => Ok(ConsoleResult {
            ok: true,
            output: out.combined().trim().to_string(),
        }),
        Err(e) => Ok(ConsoleResult {
            ok: false,
            output: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_support::{state_with, MockAdb};

    #[tokio::test]
    async fn returns_combined_output() {
        let state = state_with(
            MockAdb::default().on_shell("getprop ro.product.model", "SHIELD Android TV"),
        );
        let r = run_shell_impl(&state, "serial", "getprop ro.product.model")
            .await
            .unwrap();
        assert!(r.ok);
        assert!(r.output.contains("SHIELD Android TV"));
    }

    #[tokio::test]
    async fn strips_pasted_adb_shell_prefix() {
        let mock = MockAdb::default();
        let log = mock.shell_log();
        let state = state_with(mock);
        run_shell_impl(&state, "serial", "adb shell pm list packages")
            .await
            .unwrap();
        assert_eq!(
            log.lock().unwrap().as_slice(),
            &["pm list packages".to_string()],
            "the leading 'adb shell ' should be stripped before sending"
        );
    }

    #[tokio::test]
    async fn rejects_empty_and_overlong() {
        let state = state_with(MockAdb::default());
        assert!(!run_shell_impl(&state, "serial", "   ").await.unwrap().ok);
        let long = "x".repeat(MAX_CMD_LEN + 1);
        assert!(!run_shell_impl(&state, "serial", &long).await.unwrap().ok);
    }
}
