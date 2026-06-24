//! Audio surround-output settings — pure helpers (commitment #1: no I/O here).
//!
//! Two `Settings.Global` keys drive HDMI surround passthrough on Android TV:
//!   - `encoded_surround_output`                  — the mode (auto/never/always/manual)
//!   - `encoded_surround_output_enabled_formats`  — CSV of `AudioFormat.ENCODING_*`
//!     integer constants, honored only in MANUAL mode.
//!
//! The host (commands/audio.rs) reads/writes the settings; this module maps and
//! validates the values so a malformed UI request can't write garbage.

use serde::{Deserialize, Serialize};

/// Surround output mode — maps to the integer in `encoded_surround_output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurroundMode {
    /// AUTO (0) — send the formats the HDMI sink reports as supported.
    Auto,
    /// NEVER (1) — force stereo; never send encoded surround (the "sanity" mode).
    Never,
    /// ALWAYS (2) — assume every format is supported. Can cause silence on a
    /// sink that lacks one, which is why the UI warns.
    Always,
    /// MANUAL (3) — only the formats listed in `*_enabled_formats`.
    Manual,
}

impl SurroundMode {
    /// The string written to `encoded_surround_output`.
    pub fn to_setting_value(self) -> &'static str {
        match self {
            SurroundMode::Auto => "0",
            SurroundMode::Never => "1",
            SurroundMode::Always => "2",
            SurroundMode::Manual => "3",
        }
    }

    /// Parse the device's current `encoded_surround_output` value.
    pub fn from_setting_value(raw: &str) -> Option<Self> {
        match raw.trim() {
            "0" => Some(SurroundMode::Auto),
            "1" => Some(SurroundMode::Never),
            "2" => Some(SurroundMode::Always),
            "3" => Some(SurroundMode::Manual),
            _ => None,
        }
    }
}

/// A surround codec the user can toggle in MANUAL mode. `id` is the
/// `AudioFormat.ENCODING_*` integer constant stored in the CSV setting.
#[derive(Debug, Clone, Serialize)]
pub struct SurroundFormat {
    pub id: u32,
    /// Stable identifier for the UI / serde.
    pub key: &'static str,
    /// Human-readable label.
    pub label: &'static str,
}

/// The codecs exposed for manual surround configuration. Integer values are the
/// `AudioFormat.ENCODING_*` constants from AOSP `AudioFormat.java`. Kept to the
/// widely-supported HDMI passthrough formats.
pub const SURROUND_FORMATS: &[SurroundFormat] = &[
    SurroundFormat {
        id: 5,
        key: "ac3",
        label: "Dolby Digital (AC-3)",
    },
    SurroundFormat {
        id: 6,
        key: "eac3",
        label: "Dolby Digital Plus (E-AC-3)",
    },
    SurroundFormat {
        id: 18,
        key: "eac3_joc",
        label: "Dolby Atmos (E-AC-3 JOC)",
    },
    SurroundFormat {
        id: 7,
        key: "dts",
        label: "DTS",
    },
    SurroundFormat {
        id: 8,
        key: "dts_hd",
        label: "DTS-HD",
    },
    SurroundFormat {
        id: 14,
        key: "truehd",
        label: "Dolby TrueHD",
    },
];

/// Is `id` a format we expose (and therefore allow writing)?
pub fn is_known_format(id: u32) -> bool {
    SURROUND_FORMATS.iter().any(|f| f.id == id)
}

/// Friendly label for a format id, if known.
pub fn format_label(id: u32) -> Option<&'static str> {
    SURROUND_FORMATS
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.label)
}

/// Keep only known ids, dedup, and sort — so the value we write is stable and
/// can't smuggle an unexpected token into the setting.
pub fn normalize_formats(ids: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = ids
        .iter()
        .copied()
        .filter(|id| is_known_format(*id))
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Build the CSV for `encoded_surround_output_enabled_formats` from a set of ids.
pub fn build_enabled_formats_csv(ids: &[u32]) -> String {
    normalize_formats(ids)
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Parse the device's current CSV value into the known format ids (unknown
/// tokens are ignored for display purposes).
pub fn parse_enabled_formats_csv(raw: &str) -> Vec<u32> {
    let ids: Vec<u32> = raw
        .split(',')
        .filter_map(|t| t.trim().parse::<u32>().ok())
        .collect();
    normalize_formats(&ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_roundtrips_through_setting_value() {
        for m in [
            SurroundMode::Auto,
            SurroundMode::Never,
            SurroundMode::Always,
            SurroundMode::Manual,
        ] {
            assert_eq!(
                SurroundMode::from_setting_value(m.to_setting_value()),
                Some(m)
            );
        }
        assert_eq!(
            SurroundMode::from_setting_value(" 3 "),
            Some(SurroundMode::Manual)
        );
        assert_eq!(SurroundMode::from_setting_value("9"), None);
        assert_eq!(SurroundMode::from_setting_value("null"), None);
    }

    #[test]
    fn normalize_filters_unknown_and_dedups_and_sorts() {
        // 99 is unknown; 5 is duplicated; order is jumbled.
        assert_eq!(normalize_formats(&[14, 5, 99, 5, 7]), vec![5, 7, 14]);
        assert!(normalize_formats(&[99, 1234]).is_empty());
    }

    #[test]
    fn csv_build_and_parse_are_consistent() {
        assert_eq!(build_enabled_formats_csv(&[6, 5, 18]), "5,6,18");
        assert_eq!(parse_enabled_formats_csv("5, 6 ,18,999"), vec![5, 6, 18]);
        assert!(build_enabled_formats_csv(&[]).is_empty());
    }

    #[test]
    fn known_format_lookup() {
        assert!(is_known_format(5));
        assert!(!is_known_format(27));
        assert_eq!(format_label(14), Some("Dolby TrueHD"));
        assert_eq!(format_label(27), None);
    }
}
