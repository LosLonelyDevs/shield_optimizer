//! Pure selection of the best release asset for a device's CPU ABI.
//!
//! The quick-installer fetches a GitHub/GitLab release's asset list (host layer,
//! network) and hands the decoded names+urls here to pick the right `.apk` for
//! the device's `ro.product.cpu.abilist`. Kept pure so the matching logic is
//! unit-tested without any network.

/// One downloadable asset from a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub name: String,
    pub url: String,
}

/// True if `name_lc` (already lowercased) looks built for `abi`.
///
/// Asset authors spell architectures many ways, so each ABI maps to several
/// needles. The 32-bit-arm case is special: a bare "arm" must NOT match an
/// "arm64"/"aarch64" asset, so it's gated on the 64-bit markers being absent.
fn asset_matches_abi(name_lc: &str, abi: &str) -> bool {
    match abi {
        "arm64-v8a" => name_lc.contains("arm64") || name_lc.contains("aarch64"),
        "armeabi-v7a" | "armeabi" => {
            name_lc.contains("armeabi")
                || name_lc.contains("armv7")
                || name_lc.contains("arm-v7")
                || (name_lc.contains("arm")
                    && !name_lc.contains("arm64")
                    && !name_lc.contains("aarch64"))
        }
        "x86_64" => {
            name_lc.contains("x86_64") || name_lc.contains("x64") || name_lc.contains("amd64")
        }
        "x86" => name_lc.contains("x86") && !name_lc.contains("x86_64"),
        _ => false,
    }
}

/// Pick the asset that best matches the device's supported ABIs.
///
/// - `abis` is `ro.product.cpu.abilist` split on commas, most-preferred first.
/// - `asset_match` (optional) is a substring every candidate must contain — used
///   to narrow a multi-app release (e.g. "smarttube_stable") down to the right
///   artifact.
///
/// Strategy, in order: filter to `.apk` assets [containing `asset_match`]; prefer
/// one built for the most-preferred ABI, then any listed ABI; then a
/// universal/noarch asset; then, if exactly one `.apk` remains, take it. Returns
/// `None` when nothing fits (the caller can fall back to a direct URL).
pub fn select_asset<'a>(
    assets: &'a [ReleaseAsset],
    abis: &[String],
    asset_match: Option<&str>,
) -> Option<&'a ReleaseAsset> {
    let match_lc = asset_match.map(|m| m.to_lowercase());
    let apks: Vec<&ReleaseAsset> = assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".apk"))
        .filter(|a| {
            match_lc
                .as_deref()
                .map_or(true, |m| a.name.to_lowercase().contains(m))
        })
        .collect();
    if apks.is_empty() {
        return None;
    }

    // Exact-ABI match, honoring device preference order.
    for abi in abis {
        if let Some(found) = apks
            .iter()
            .find(|a| asset_matches_abi(&a.name.to_lowercase(), abi))
        {
            return Some(found);
        }
    }

    // Universal / noarch fallback.
    if let Some(found) = apks.iter().find(|a| {
        let n = a.name.to_lowercase();
        n.contains("universal")
            || n.contains("noarch")
            || n.contains("-all.")
            || n.contains("_all.")
    }) {
        return Some(found);
    }

    // A single candidate with no arch markers (common: one universal apk).
    if apks.len() == 1 {
        return Some(apks[0]);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_string(),
            url: format!("https://example.com/{name}"),
        }
    }

    fn abis(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arm64_device_prefers_arm64_asset() {
        let assets = vec![
            asset("app-armeabi-v7a-release.apk"),
            asset("app-arm64-v8a-release.apk"),
            asset("app-x86_64-release.apk"),
        ];
        let pick = select_asset(&assets, &abis(&["arm64-v8a", "armeabi-v7a"]), None).unwrap();
        assert_eq!(pick.name, "app-arm64-v8a-release.apk");
    }

    #[test]
    fn arm32_only_device_does_not_match_arm64_asset() {
        let assets = vec![asset("app-arm64-v8a.apk"), asset("app-armeabi-v7a.apk")];
        let pick = select_asset(&assets, &abis(&["armeabi-v7a"]), None).unwrap();
        assert_eq!(pick.name, "app-armeabi-v7a.apk");
    }

    #[test]
    fn bare_arm_asset_matches_arm32_not_arm64() {
        // TizenTube/Cobalt-style naming: "arm64" vs bare "arm".
        let assets = vec![asset("player-arm64.apk"), asset("player-arm.apk")];
        assert_eq!(
            select_asset(&assets, &abis(&["arm64-v8a"]), None)
                .unwrap()
                .name,
            "player-arm64.apk"
        );
        assert_eq!(
            select_asset(&assets, &abis(&["armeabi-v7a"]), None)
                .unwrap()
                .name,
            "player-arm.apk"
        );
    }

    #[test]
    fn asset_match_filters_multi_app_release() {
        let assets = vec![
            asset("smarttube_beta_30.apk"),
            asset("smarttube_stable_30.apk"),
        ];
        let pick = select_asset(&assets, &abis(&["arm64-v8a"]), Some("smarttube_stable")).unwrap();
        assert_eq!(pick.name, "smarttube_stable_30.apk");
    }

    #[test]
    fn falls_back_to_universal() {
        let assets = vec![
            asset("app-arm64-v8a-release.apk"),
            asset("app-universal-release.apk"),
        ];
        // A device whose ABI isn't represented gets the universal build.
        let pick = select_asset(&assets, &abis(&["mips"]), None).unwrap();
        assert_eq!(pick.name, "app-universal-release.apk");
    }

    #[test]
    fn single_apk_is_taken() {
        let assets = vec![asset("AuroraStore_4.5.1.apk"), asset("source.zip")];
        let pick = select_asset(&assets, &abis(&["arm64-v8a"]), None).unwrap();
        assert_eq!(pick.name, "AuroraStore_4.5.1.apk");
    }

    #[test]
    fn no_apk_returns_none() {
        let assets = vec![asset("release-notes.txt"), asset("source.tar.gz")];
        assert!(select_asset(&assets, &abis(&["arm64-v8a"]), None).is_none());
    }

    #[test]
    fn x86_64_does_not_match_x86() {
        let assets = vec![asset("app-x86.apk"), asset("app-x86_64.apk")];
        assert_eq!(
            select_asset(&assets, &abis(&["x86_64"]), None)
                .unwrap()
                .name,
            "app-x86_64.apk"
        );
        assert_eq!(
            select_asset(&assets, &abis(&["x86"]), None).unwrap().name,
            "app-x86.apk"
        );
    }
}
