//! scrcpy screen mirroring — launches the scrcpy desktop client as a separate
//! "View & Control" window.
//!
//! Unlike the Remote tab (which drives the device headlessly over the bundled
//! scrcpy *server* control channel), this opens scrcpy's own GUI. scrcpy is a
//! separate ~40 MB binary per OS, so rather than bundle it we launch it from
//! PATH and, if it isn't installed, return actionable guidance. The spawned
//! process is detached so it keeps running after this call returns.

use std::path::PathBuf;

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

    let mut cmd = std::process::Command::new(bin);
    cmd.args(["--serial", &serial]);
    // Don't flash a console window on Windows when launching from the GUI.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    // Spawn detached — the returned Child is dropped, leaving scrcpy running in
    // its own window until the user closes it.
    match cmd.spawn() {
        Ok(_child) => Ok(ActionResult {
            ok: true,
            message: "Opening scrcpy mirror window… close it on your desktop when done."
                .to_string(),
        }),
        Err(e) => Ok(ActionResult {
            ok: false,
            message: format!("Couldn't launch scrcpy: {e}"),
        }),
    }
}
