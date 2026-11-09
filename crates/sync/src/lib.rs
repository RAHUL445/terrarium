//! Device-to-device sync for one space.
//!
//! Each device publishes its change log as numbered batches,
//! `devices/<device>/<seq>.batch`, into a [`BatchStore`]. A batch is
//! encrypted with the space key (XChaCha20-Poly1305) and signed with the
//! sender's device key (Ed25519). A store only ever sees ciphertext, device
//! ids, sequence numbers and sizes.
//!
//! A device re-publishes changes it received from others too (its whole log
//! in local order), so any single batch stream delivers changes after their
//! ancestors. Merging is done by `terrarium-core` on each device.

mod backup;
mod drive;
mod engine;
mod envelope;
mod error;
mod http;
mod http_auth;
mod keys;
/// Staged protocol primitives; live sync still uses the v1 membership path.
pub mod membership;
pub mod pairing;
mod peer;
mod transport;

pub use backup::{Backup, BackupInfo, RestoreReport, BACKUP_EXT};
pub use drive::{
    authorize as authorize_drive, DriveCredentials, DriveEndpoints, DriveStore,
    SCOPE as DRIVE_SCOPE,
};
pub use engine::{Rejected, SpaceSync, SyncReport, SyncState};
pub use envelope::{Envelope, Kind, Payload};
pub use error::{Result, SyncError};
pub use http::{pair_with, HttpStore};
pub use keys::{DeviceKeys, DevicePublic, Roster, SpaceKey, WrappedKey};
pub use peer::{PeerServer, Relay};
pub use transport::{BatchStore, FolderStore, MemoryStore, MultiStore};
