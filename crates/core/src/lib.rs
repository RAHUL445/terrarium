//! Core of the local-first plugin host.
//!
//! Phase 1 scope: per-plugin SQLite storage where every write goes through
//! the host, lands in a change log stamped with a hybrid logical clock, and
//! merges deterministically on every device using per-column merge rules
//! and per-table delete policies. See `docs/develop/architecture.md`.

pub mod change;
pub mod error;
pub mod hlc;
mod read_policy;
pub mod schema;
pub mod store;
mod text;
mod value;

pub use change::{Batch, Change, Op, VersionVector};
pub use error::{Error, Result};
pub use hlc::{Clock, DeviceId, Hlc, ManualTime, SystemTime, TimeSource};
pub use schema::{
    ColumnDef, ColumnType, DeletePolicy, MergeRule, PluginSchema, TableDef, TableKind,
};
pub use store::{
    ChangeRecord, ConflictRecord, ImportReport, NoticeRecord, RowState, Store, StoreConfig,
    StoreSet, Writer,
};
