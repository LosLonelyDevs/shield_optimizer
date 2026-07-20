//! scrcpy screen mirroring — launches the scrcpy desktop client as a separate
//! "View & Control" window.
//!
//! Unlike the Remote tab (which drives the device headlessly over the bundled
//! scrcpy *server* control channel), this opens scrcpy's own GUI. scrcpy is a
//! separate ~40 MB binary per OS, so rather than bundle it we launch it from
//! PATH and, if it isn't installed, return actionable guidance. The spawned
//! process is detached so it keeps running after this call returns; its
//! stdout/stderr are captured into the activity log so a failed launch is
//! diagnosable from the Console tab instead of dying invisibly.

use std::path::PathBuf;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::activity_log;

use super::apps::ActionResult;

/// Locate a `scrcpy` executable on PATH.
fn find_scrcpy() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["scrcpy.exe"]
    } else {
        &["scrcpy"]
    };
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

async fn slurp<R: AsyncRead + Unpin>(stream: Option<R>) -> String {
    let Some(mut stream) = stream else {
        return String::new();
    };
    let mut buf = String::new();
    let _ = stream.read_to_string(&mut buf).await;
    buf
}

/// `mirror_screen` — open a scrcpy mirror window for `serial`. Returns
/// actionable guidance if scrcpy isn't installed (the fast Remote tab works
/// without it).
#[tauri::command]
pub async fn mirror_screen(serial: String) -> Result<ActionResult, String> {
    let Some(bin) = find_scrcpy() else {
        return Ok(ActionResult {
            ok: false,
            message: "scrcpy isn't on your PATH. Install it (macOS: `brew install scrcpy`, \
                      Windows: `choco install scrcpy` or scoop, Linux: your package manager — \
                      or https://github.com/Genymobile/scrcpy), then retry. The Remote tab works \
                      without it."
                .to_string(),
        });
    };

    // --audio-codec=aac: scrcpy 2+ defaults audio to Opus, but Android TV
    // builds (the SHIELD's Android 11 included) often ship no Opus encoder,
    // and a failed audio setup aborts the whole scrcpy session. Every Android
    // build has an AAC encoder (CDD-mandated), so pin it.
    let args = ["--serial", serial.as_str(), "--audio-codec=aac"];
    let rendered = activity_log::render_command("scrcpy", &args);

    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Don't flash a console window on Windows when launching from the GUI.
    crate::adb::hide_console_window(&mut cmd);

    // Spawn detached (no kill_on_drop) — scrcpy keeps its own window open
    // until the user closes it. A background task drains both pipes for the
    // child's lifetime (so it can never block on a full pipe) and records the
    // output when it exits — that's where "no Opus encoder"-style failures
    // become visible.
    match cmd.spawn() {
        Ok(mut child) => {
            activity_log::record("scrcpy", rendered.clone(), "(launched)", true);
            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            tauri::async_runtime::spawn(async move {
                let (out, err) = tokio::join!(slurp(stdout), slurp(stderr));
                let status = child.wait().await;
                let ok = matches!(&status, Ok(s) if s.success());
                let mut combined = format!("{out}\n{err}").trim().to_string();
                if combined.is_empty() {
                    combined = "(no output)".to_string();
                }
                activity_log::record("scrcpy", rendered, &combined, ok);
            });
            Ok(ActionResult {
                ok: true,
                message: "Opening scrcpy mirror window… if it doesn't appear, check the \
                          Console tab's Background activity for the error."
                    .to_string(),
            })
        }
        Err(e) => {
            activity_log::record("scrcpy", rendered, &e.to_string(), false);
            Ok(ActionResult {
                ok: false,
                message: format!("Couldn't launch scrcpy: {e}"),
            })
        }
    }
}
