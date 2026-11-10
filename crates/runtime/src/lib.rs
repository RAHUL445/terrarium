//! Runs plugins: reads manifests, compiles Wasm components, executes
//! operations under fuel, time and memory limits, and serves the
//! manifest's named queries and UI templates.
//!
//! A plugin operation runs in a fresh component instance inside one storage
//! transaction: it commits only if the operation returns `ok`.

mod assistant;
mod builder;
pub mod commands;
mod error;
mod host;
mod manifest;
mod package;
mod packages;
mod pages;
mod registry;
pub mod services;
mod spaces;
mod speech;
mod theme;
mod time;

pub use builder::{build_plugin, componentize};
pub use error::{Result, RuntimeError};
pub use host::Execution;
pub use manifest::{
    Effect, InterfaceDef, JobDef, Limits, Manifest, OpenDef, OperationDef, ParamSpec, ParamType,
    QueryDef, QuickAction, RemindersDef, RunOn, Schedule, ViewDef, API_VERSION, CONTEXT_PARAMS,
    ICONS, PERMISSIONS,
};
pub use package::{
    added_capabilities, capabilities, default_signing_key_path, generate_signing_key,
    key_fingerprint, read_signing_key, signing_public_key, Capability, Package, Signature,
    MAX_PACKAGE_BYTES, PACKAGE_EXT,
};
pub use packages::{Installed, PackageSource, Packages, BUNDLED_DIGEST};
pub use pages::{
    clean_config, clean_host_config, home_id, host_widgets, PageInfo, Pages, Placement,
    HOST_WIDGETS, MAX_PLACEMENTS, PAGES_STORE_ID,
};
pub use registry::{fingerprint, ChangeSummary, HostConfig, PluginHost, ViewKind};
pub use spaces::{split_store_id, store_id, SpaceInfo, Spaces, DEFAULT_SPACE, SPACES_STORE_ID};
pub use speech::SpeechConfig;
/// The AI service and scheduler crates, for hosts.
pub use terrarium_ai as ai;
pub use terrarium_scheduler as scheduler;
pub use theme::{contrast, Theme, THEME_EXT, THEME_FONTS, THEME_TOKENS};
