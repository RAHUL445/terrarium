//! Runs plugins: reads manifests, compiles Wasm components, executes
//! operations under fuel, time and memory limits, and serves the
//! manifest's named queries and UI templates.
//!
//! A plugin operation runs in a fresh component instance inside one storage
//! transaction: it commits only if the operation returns `ok`.

mod assistant;
mod builder;
pub mod commands;
mod csv;
mod error;
mod export;
mod host;
mod import;
mod index;
mod links;
mod manifest;
mod package;
mod packages;
mod pages;
mod refs;
mod registry;
pub mod services;
mod spaces;
mod speech;
mod theme;
mod time;

pub use builder::{build_plugin, componentize};
pub use error::{Result, RuntimeError};
pub use host::Execution;
pub use index::{IndexProgress, INDEX_CHUNK};
pub use links::{LinkInfo, Links, LINKS_STORE_ID, LINK_KINDS};
pub use manifest::{
    Effect, ExportDef, ExportFormat, ImportDef, ImportFormat, IndexDef, InterfaceDef, JobDef,
    Limits, Manifest, MoveDef, OpenDef, OperationDef, ParamSpec, ParamType, QueryDef, QueryView,
    QuickAction, RefDef, RemindersDef, RunOn, Schedule, ShareTarget, ViewColumn, ViewDef,
    API_VERSION, CONTEXT_PARAMS, ICONS, MAX_INDEX_ENTRIES, PERMISSIONS, SHARE_TYPES,
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
pub use registry::{
    fingerprint, ChangeSummary, HostConfig, PluginHost, ViewKind, INDEX_STEP_ROWS,
    SEARCH_CATCH_UP_ROWS,
};
pub use spaces::{split_store_id, store_id, SpaceInfo, Spaces, DEFAULT_SPACE, SPACES_STORE_ID};
pub use speech::SpeechConfig;
/// The AI service and scheduler crates, for hosts.
pub use terrarium_ai as ai;
pub use terrarium_scheduler as scheduler;
pub use theme::{contrast, Theme, THEME_EXT, THEME_FONTS, THEME_TOKENS};

pub use refs::{ItemRef, MAX_RESOLVE};
mod bulk;
