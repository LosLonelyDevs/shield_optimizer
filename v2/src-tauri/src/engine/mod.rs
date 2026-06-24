//! Engine — pure logic with no I/O.
//!
//! Per architectural commitment #1 (see `v2/README.md`): nothing in this module
//! should call ADB, read files, or make network requests. The engine returns
//! plans that the host layer (Tauri commands) is responsible for executing
//! through the ADB driver.

pub mod app_lists;
pub mod audio;
pub mod detection;
pub mod kodi;
pub mod launcher;
pub mod optimize;
pub mod release_assets;
pub mod safety;
pub mod snapshot;
pub mod types;

pub use app_lists::{AppList, AppListBundle};
pub use audio::{
    build_enabled_formats_csv, parse_enabled_formats_csv, SurroundFormat, SurroundMode,
    SURROUND_FORMATS,
};
pub use detection::{detect_device_type, DeviceType};
pub use kodi::{render_advancedsettings, AdjustRefreshRate, KodiProfile};
pub use launcher::{
    is_last_enabled_home_handler, is_valid_package_name, launcher_catalog, launcher_rows,
    stock_launcher_catalog, LauncherEntry, LauncherStatus,
};
pub use optimize::{compute_plan, OptimizeInputs, OptimizePlan};
pub use release_assets::{select_asset, ReleaseAsset};
pub use safety::{classify as classify_safety, is_never_disable, Safety};
pub use snapshot::{Snapshot, SnapshotApplyPlan, SnapshotError, SCHEMA_VERSION};
pub use types::{
    ActionMethod, AppEntry, Device, DeviceProperties, DeviceStatus, OptimizeAction, OptimizeMode,
    OptimizePlanItem, RiskTier, SkipReason,
};
