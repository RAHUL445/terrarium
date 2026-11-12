//! Profile-owned files and WebView preferences. Release defaults stay unchanged.
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};

pub fn profile_dir() -> Result<Option<PathBuf>, String> {
    let profile = std::env::var_os("TERRA_PROFILE_DIR").map(PathBuf::from);
    if profile.as_ref().is_some_and(|path| !path.is_absolute()) {
        return Err("TERRA_PROFILE_DIR must be an absolute path".into());
    }
    Ok(profile)
}

pub fn root(app: &AppHandle) -> Result<PathBuf, String> {
    match profile_dir()? {
        Some(profile) => Ok(profile),
        None => app.path().app_data_dir().map_err(crate::err),
    }
}

/// Stable across launches, distinct for each profile and the default debug UI.
/// This is an identifier, not a security hash.
pub fn webview_id(profile: Option<&Path>, debug: bool) -> Option<[u8; 16]> {
    if profile.is_none() && !debug {
        return None;
    }
    let name = profile
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "terrarium-debug".into());
    let mut id = [0; 16];
    for (i, seed) in [0xcbf29ce484222325u64, 0x84222325cbf29ce4]
        .into_iter()
        .enumerate()
    {
        let hash = name.bytes().fold(seed, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
        id[i * 8..(i + 1) * 8].copy_from_slice(&hash.to_le_bytes());
    }
    Some(id)
}

pub fn preferences_script(id: Option<[u8; 16]>) -> String {
    let Some(id) = id else { return String::new() };
    let prefix: String = id.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "window.terrariumProfileId = \"{prefix}\";\n{}",
        include_str!("../../../../ui/profile-storage.js")
    )
}

#[derive(serde::Serialize)]
pub struct BackupLocation {
    pub directory: PathBuf,
    pub notice: Option<String>,
}

const BACKUP_HELP: &str = "Terrarium can't use the Documents backup folder. Allow Documents access in System Settings → Privacy & Security → Files and Folders.";

pub fn backup_location(app: &AppHandle) -> Result<BackupLocation, String> {
    let profile = profile_dir()?;
    let documents = if profile.is_none() && cfg!(desktop) {
        app.path()
            .document_dir()
            .ok()
            .map(|dir| dir.join("Terrarium Backups"))
    } else {
        None
    };
    choose_backup_location(profile.as_deref(), documents.as_deref(), &root(app)?)
}

fn writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // An existing directory can still deny writes. Probe before choosing it.
    tempfile::NamedTempFile::new_in(dir)?;
    Ok(())
}

fn choose_backup_location(
    profile: Option<&Path>,
    documents: Option<&Path>,
    app_data: &Path,
) -> Result<BackupLocation, String> {
    if let Some(profile) = profile {
        let directory = profile.join("backups");
        writable(&directory).map_err(|_| "Terrarium can't save backups in this profile. Check the profile folder's permissions.".to_string())?;
        return Ok(BackupLocation {
            directory,
            notice: None,
        });
    }
    if let Some(directory) = documents {
        if writable(directory).is_ok() {
            return Ok(BackupLocation {
                directory: directory.into(),
                notice: None,
            });
        }
    }
    let directory = app_data.join("backups");
    writable(&directory).map_err(|_| {
        format!(
            "{BACKUP_HELP} The app data backup folder is also unavailable; check its permissions."
        )
    })?;
    Ok(BackupLocation {
        directory,
        notice: cfg!(desktop)
            .then(|| format!("{BACKUP_HELP} Backups are stored in app data instead.")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_profile_never_touches_documents() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("profile");
        let docs = tmp.path().join("documents");
        let real = tmp.path().join("real");
        let location = choose_backup_location(Some(&profile), Some(&docs), &real).unwrap();
        assert_eq!(location.directory, profile.join("backups"));
        assert!(!docs.exists());
        assert!(!real.exists());
    }

    #[test]
    fn unavailable_documents_falls_back_even_if_directory_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("blocked");
        std::fs::write(&docs, b"not a directory").unwrap();
        let location = choose_backup_location(None, Some(&docs), tmp.path()).unwrap();
        assert_eq!(location.directory, tmp.path().join("backups"));
        assert!(location.notice.unwrap().contains("Files and Folders"));
    }

    #[test]
    fn unavailable_profile_does_not_fall_back_to_real_data() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join("blocked-profile");
        std::fs::write(&profile, b"not a directory").unwrap();
        let real = tmp.path().join("real");
        let error = choose_backup_location(Some(&profile), None, &real)
            .err()
            .unwrap();
        assert!(error.contains("profile folder's permissions"));
        assert!(!real.exists());
    }

    #[test]
    fn unavailable_backup_folders_explain_recovery_without_os_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let error = choose_backup_location(None, Some(&blocked), &blocked)
            .err()
            .unwrap();
        assert!(error.contains("Files and Folders"));
        assert!(error.contains("app data backup folder is also unavailable"));
        assert!(!error.contains("os error"));
    }

    #[test]
    fn release_documents_location_is_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("Documents");
        let location = choose_backup_location(None, Some(&docs), tmp.path()).unwrap();
        assert_eq!(location.directory, docs);
        assert!(location.notice.is_none());
    }

    #[test]
    fn webview_profiles_are_stable_and_separate_from_release_and_debug() {
        let a = Some(Path::new("/tmp/profile-a"));
        let b = Some(Path::new("/tmp/profile-b"));
        assert_eq!(webview_id(None, false), None);
        assert_eq!(webview_id(a, false), webview_id(a, true));
        assert_ne!(webview_id(a, true), webview_id(b, true));
        assert_ne!(webview_id(a, true), webview_id(None, true));
        assert!(!preferences_script(webview_id(a, true)).is_empty());
        assert!(preferences_script(None).is_empty());
    }
}
