//! One process at a time writes a data folder: the app, or `terrarium
//! serve`. Each takes an exclusive lock on `terrarium.lock` in the data
//! folder for as long as it runs; the system releases it when the process
//! ends, however it ends.
//!
//! The app takes over from `terrarium serve`: it leaves a hand-off file,
//! serve sees it, stops between commands and exits, and the app then gets
//! the lock. Nothing is signalled by process id.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const LOCK_FILE: &str = "terrarium.lock";
const HANDOFF_FILE: &str = "terrarium.handoff";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The app with its window.
    App,
    /// `terrarium serve`.
    Serve,
}

#[derive(Serialize, Deserialize)]
struct Holder {
    role: Role,
    pid: u32,
}

/// Held while this process writes the data folder.
#[derive(Debug)]
pub struct DataLock {
    _file: File,
}

#[derive(Debug)]
pub enum LockError {
    /// Another process holds it; its role when it said.
    Busy(Option<Role>),
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Busy(Some(Role::App)) => write!(f, "Terrarium is already open."),
            LockError::Busy(Some(Role::Serve)) => {
                write!(f, "terrarium serve is already running.")
            }
            LockError::Busy(None) => write!(f, "Another copy of Terrarium is using this data."),
            LockError::Io(e) => write!(f, "Could not lock the data folder: {e}"),
        }
    }
}

fn open(dir: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    // Never truncate before holding the lock: the holder's role is in it.
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(dir.join(LOCK_FILE))
}

/// Takes the lock on `dir` for `role`, or says who has it.
pub fn acquire(dir: &Path, role: Role) -> Result<DataLock, LockError> {
    let mut file = open(dir).map_err(LockError::Io)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let mut text = String::new();
            let _ = file.read_to_string(&mut text);
            let holder = serde_json::from_str::<Holder>(&text).ok();
            return Err(LockError::Busy(holder.map(|h| h.role)));
        }
        Err(TryLockError::Error(e)) => return Err(LockError::Io(e)),
    }
    let holder = serde_json::to_vec(&Holder {
        role,
        pid: std::process::id(),
    })
    .unwrap_or_default();
    file.set_len(0)
        .and_then(|()| file.seek(SeekFrom::Start(0)))
        .and_then(|_| file.write_all(&holder))
        .map_err(LockError::Io)?;
    Ok(DataLock { _file: file })
}

/// For the app: takes the lock, asking a running `terrarium serve` to hand
/// over and waiting up to `wait` for it to stop.
pub fn take_over(dir: &Path, wait: Duration) -> Result<DataLock, LockError> {
    let handoff = handoff_path(dir);
    let started = Instant::now();
    let result = loop {
        match acquire(dir, Role::App) {
            Err(LockError::Busy(Some(Role::Serve))) if started.elapsed() < wait => {
                if !handoff.exists() {
                    eprintln!("asking terrarium serve to hand over");
                    let _ = std::fs::write(&handoff, b"");
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            other => break other,
        }
    };
    let _ = std::fs::remove_file(&handoff);
    result
}

fn handoff_path(dir: &Path) -> PathBuf {
    dir.join(HANDOFF_FILE)
}

/// For `terrarium serve`: whether the app has asked to take over.
pub fn handoff_requested(dir: &Path) -> bool {
    handoff_path(dir).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_holder_at_a_time() {
        let dir = tempfile::tempdir().unwrap();
        let held = acquire(dir.path(), Role::Serve).unwrap();
        assert!(matches!(
            acquire(dir.path(), Role::Serve),
            Err(LockError::Busy(Some(Role::Serve)))
        ));
        assert!(matches!(
            acquire(dir.path(), Role::App),
            Err(LockError::Busy(Some(Role::Serve)))
        ));
        drop(held);
        let app = acquire(dir.path(), Role::App).unwrap();
        assert!(matches!(
            acquire(dir.path(), Role::Serve),
            Err(LockError::Busy(Some(Role::App)))
        ));
        drop(app);
    }

    #[test]
    fn the_app_takes_over_from_serve() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let held = acquire(&path, Role::Serve).unwrap();
        let serve = std::thread::spawn(move || {
            while !handoff_requested(&path) {
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(held);
        });
        let app = take_over(dir.path(), Duration::from_secs(5)).unwrap();
        serve.join().unwrap();
        assert!(!handoff_requested(dir.path()));
        drop(app);
    }

    #[test]
    fn the_app_does_not_take_over_from_another_app() {
        let dir = tempfile::tempdir().unwrap();
        let _held = acquire(dir.path(), Role::App).unwrap();
        assert!(matches!(
            take_over(dir.path(), Duration::from_secs(5)),
            Err(LockError::Busy(Some(Role::App)))
        ));
        assert!(!handoff_requested(dir.path()));
    }
}
