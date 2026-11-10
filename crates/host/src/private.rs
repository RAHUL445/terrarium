//! Owner-only files written atomically: client settings, keys, state.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    // NamedTempFile creates an exclusive random file with owner-only access;
    // secret bytes are never written through a predictable path or symlink.
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn read_private(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(err(e)),
    };
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err("Secret storage must be a regular file no larger than 1 MiB".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(err)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(err)?;
    if bytes.len() > 1024 * 1024 {
        return Err("Secret storage is too large".into());
    }
    Ok(Some(bytes))
}
