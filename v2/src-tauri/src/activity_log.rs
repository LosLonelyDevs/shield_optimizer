//! In-memory log of every subprocess the app runs behind the scenes — adb
//! invocations and scrcpy launches. The Console tab's "Background activity"
//! section tails it so users can see exactly what ran and what it printed
//! without attaching a debugger (e.g. why a scrcpy window died before it ever
//! appeared).
//!
//! A fixed-capacity ring buffer behind a global mutex. Writers are the adb
//! driver and the mirror command — the driver has no access to Tauri state,
//! hence the global rather than a field on `AppState`. The reader is the
//! `activity_tail` command, which polls incrementally by entry id.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Max entries retained; oldest are dropped first.
const CAPACITY: usize = 500;

/// Per-entry output cap. The *tail* is kept — the end of the output is where
/// errors land.
const MAX_OUTPUT: usize = 8_000;

#[derive(Debug, Clone, Serialize)]
pub struct ActivityEntry {
    /// Monotonic id — the frontend polls "everything after id N".
    pub id: u64,
    /// Milliseconds since the Unix epoch; the frontend formats it.
    pub ts_ms: u64,
    /// Which subsystem ran it: "adb" or "scrcpy".
    pub source: String,
    /// The command line as launched.
    pub command: String,
    /// Combined stdout/stderr (tail-truncated), or a status note.
    pub output: String,
    pub ok: bool,
}

#[derive(Default)]
struct Ring {
    next_id: u64,
    entries: VecDeque<ActivityEntry>,
}

impl Ring {
    fn push(&mut self, source: &str, command: String, output: &str, ok: bool) {
        self.next_id += 1;
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.entries.push_back(ActivityEntry {
            id: self.next_id,
            ts_ms,
            source: source.to_string(),
            command,
            output: tail_truncate(output, MAX_OUTPUT),
            ok,
        });
        while self.entries.len() > CAPACITY {
            self.entries.pop_front();
        }
    }

    fn tail(&self, after_id: u64) -> Vec<ActivityEntry> {
        self.entries
            .iter()
            .filter(|e| e.id > after_id)
            .cloned()
            .collect()
    }
}

fn tail_truncate(s: &str, max: usize) -> String {
    let trimmed = s.trim();
    if trimmed.len() <= max {
        return trimmed.to_string();
    }
    let mut start = trimmed.len() - max;
    while !trimmed.is_char_boundary(start) {
        start += 1;
    }
    format!("… {}", &trimmed[start..])
}

fn ring() -> &'static Mutex<Ring> {
    static RING: OnceLock<Mutex<Ring>> = OnceLock::new();
    RING.get_or_init(Mutex::default)
}

/// Append one entry. Best-effort: a poisoned lock just drops the entry.
pub fn record(source: &str, command: String, output: &str, ok: bool) {
    if let Ok(mut r) = ring().lock() {
        r.push(source, command, output, ok);
    }
}

/// Entries with id > `after_id`, oldest first.
pub fn tail(after_id: u64) -> Vec<ActivityEntry> {
    ring().lock().map(|r| r.tail(after_id)).unwrap_or_default()
}

/// Render `program + args` as a copy-pasteable line; args with spaces quoted.
pub fn render_command(program: &str, args: &[&str]) -> String {
    let mut out = String::from(program);
    for a in args {
        out.push(' ');
        if a.is_empty() || a.contains(' ') {
            out.push('"');
            out.push_str(a);
            out.push('"');
        } else {
            out.push_str(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_returns_only_newer_entries_in_order() {
        let mut r = Ring::default();
        r.push("adb", "adb devices".into(), "ok", true);
        r.push("adb", "adb shell ls".into(), "files", true);
        r.push("scrcpy", "scrcpy --serial x".into(), "boom", false);

        let all = r.tail(0);
        assert_eq!(all.len(), 3);
        assert!(all.windows(2).all(|w| w[0].id < w[1].id));

        let newer = r.tail(all[1].id);
        assert_eq!(newer.len(), 1);
        assert_eq!(newer[0].command, "scrcpy --serial x");
        assert!(!newer[0].ok);
    }

    #[test]
    fn capacity_drops_oldest() {
        let mut r = Ring::default();
        for i in 0..(CAPACITY + 10) {
            r.push("adb", format!("cmd {i}"), "", true);
        }
        assert_eq!(r.entries.len(), CAPACITY);
        // Ids keep growing even as old entries fall off the front.
        assert_eq!(r.tail(0).first().unwrap().command, "cmd 10");
    }

    #[test]
    fn output_keeps_the_tail_when_truncated() {
        let long = format!("{}THE-ERROR", "x".repeat(MAX_OUTPUT * 2));
        let mut r = Ring::default();
        r.push("adb", "cmd".into(), &long, false);
        let out = &r.tail(0)[0].output;
        assert!(out.ends_with("THE-ERROR"));
        assert!(out.starts_with('…'));
        assert!(out.len() < long.len());
    }

    #[test]
    fn render_command_quotes_spaced_args() {
        assert_eq!(
            render_command("adb", &["-s", "serial", "shell", "pm list packages"]),
            r#"adb -s serial shell "pm list packages""#
        );
    }
}
