use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_BLOB: u64 = 25 * 1024 * 1024;
const MAX_SEALED: u64 = MAX_BLOB + 4096;
const MAGIC: &[u8; 4] = b"TBL1";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 53;
const ENC_INFO: &[u8] = b"terrarium/blobs/v1";
const ID_INFO: &[u8] = b"terrarium/blob-id/v1";
const TMP_TTL: Duration = Duration::from_secs(60 * 60);

pub type Result<T> = std::result::Result<T, BlobError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BlobError {
    #[error("blob is larger than {MAX_BLOB} bytes")]
    TooLarge,
    #[error("blob not found")]
    NotFound,
    #[error("blob is corrupt")]
    Corrupt,
    #[error("no key for this blob")]
    NoKey,
    #[error("{0}")]
    Io(String),
}

#[derive(Clone, PartialEq, Eq)]
pub struct BlobKey {
    enc: [u8; 32],
    id: [u8; 32],
    key_id: String,
}

impl fmt::Debug for BlobKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobKey({})", self.key_id)
    }
}

impl BlobKey {
    pub fn new(kind: &str, root: &[u8; 32]) -> BlobKey {
        let mut enc = [0u8; 32];
        let mut id = [0u8; 32];
        Hkdf::<Sha256>::new(None, root)
            .expand(ENC_INFO, &mut enc)
            .expect("32 bytes is a valid HKDF output length");
        Hkdf::<Sha256>::new(None, root)
            .expand(ID_INFO, &mut id)
            .expect("32 bytes is a valid HKDF output length");
        let _ = kind;
        let fingerprint = Sha256::digest(id);
        let key_id = hex(&fingerprint[..4]);
        BlobKey { enc, id, key_id }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }
}

pub use terrarium_core::BlobId;

pub struct BlobStore {
    dir: PathBuf,
    key: Option<BlobKey>,
}

impl BlobStore {
    pub fn open(dir: &Path) -> Result<BlobStore> {
        fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        set_private_dir_permissions(dir)?;
        let store = BlobStore {
            dir: dir.to_path_buf(),
            key: None,
        };
        store.clean_temp_files_at(SystemTime::now())?;
        Ok(store)
    }

    pub fn set_key(&mut self, key: BlobKey) {
        self.key = Some(key);
    }

    pub fn id_of(&self, plain: &[u8]) -> Result<BlobId> {
        let key = self.key.as_ref().ok_or(BlobError::NoKey)?;
        let mac = hmac_sha256(&key.id, plain);
        format!("b1-{}-{}", key.key_id(), hex(&mac))
            .parse()
            .map_err(|_| BlobError::Corrupt)
    }

    pub fn put(&mut self, plain: &[u8]) -> Result<BlobId> {
        if plain.len() as u64 > MAX_BLOB {
            return Err(BlobError::TooLarge);
        }
        let id = self.id_of(plain)?;
        if self.has(&id) {
            return Ok(id);
        }
        let sealed = self.seal(&id, plain)?;
        self.write_blob(&id, &sealed)?;
        Ok(id)
    }

    pub fn put_sealed(&mut self, id: &BlobId, sealed: &[u8]) -> Result<()> {
        if sealed.len() as u64 > MAX_SEALED {
            return Err(BlobError::TooLarge);
        }
        if self.has(id) {
            return Ok(());
        }
        let plain = self.open_sealed(id, sealed)?;
        let computed = self.id_of(&plain)?;
        if &computed != id {
            return Err(BlobError::Corrupt);
        }
        self.write_blob(id, sealed)
    }

    pub fn get(&self, id: &BlobId) -> Result<Vec<u8>> {
        let sealed = self.read_blob(id)?;
        let plain = self.open_sealed(id, &sealed)?;
        let computed = self.id_of(&plain)?;
        if computed != *id {
            return Err(BlobError::Corrupt);
        }
        Ok(plain)
    }

    pub fn get_sealed(&self, id: &BlobId) -> Result<Vec<u8>> {
        self.read_blob(id)
    }

    pub fn has(&self, id: &BlobId) -> bool {
        self.file_path(id).is_file()
    }

    pub fn size(&self, id: &BlobId) -> Result<u64> {
        let path = self.file_path(id);
        let meta = fs::metadata(&path).map_err(|e| map_read_error(&path, e))?;
        Ok(meta.len())
    }

    pub fn list(&self) -> Vec<BlobId> {
        let mut out = Vec::new();
        let Ok(kids) = fs::read_dir(&self.dir) else {
            return out;
        };
        for kid in kids.flatten() {
            let kid_path = kid.path();
            if !kid_path.is_dir() {
                continue;
            }
            let Ok(prefixes) = fs::read_dir(&kid_path) else {
                continue;
            };
            for prefix in prefixes.flatten() {
                let prefix_path = prefix.path();
                if !prefix_path.is_dir() {
                    continue;
                }
                let Ok(files) = fs::read_dir(&prefix_path) else {
                    continue;
                };
                for file in files.flatten() {
                    let path = file.path();
                    if path.extension().and_then(|s| s.to_str()) != Some("blob") {
                        continue;
                    }
                    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if let Ok(id) = stem.parse() {
                        out.push(id);
                    }
                }
            }
        }
        out.sort();
        out
    }

    pub fn remove(&mut self, id: &BlobId) -> Result<()> {
        let path = self.file_path(id);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(&path, e)),
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.list()
            .into_iter()
            .filter_map(|id| self.size(&id).ok())
            .sum()
    }

    fn file_path(&self, id: &BlobId) -> PathBuf {
        self.dir
            .join(id.kid8())
            .join(id.mac_first2())
            .join(format!("{}.blob", id.as_str()))
    }

    fn write_blob(&self, id: &BlobId, sealed: &[u8]) -> Result<()> {
        let path = self.file_path(id);
        let parent = path
            .parent()
            .ok_or_else(|| BlobError::Io("invalid path".into()))?;
        fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        set_private_dir_permissions(parent)?;
        let mut tmp = tempfile::Builder::new()
            .suffix(".tmp")
            .tempfile_in(parent)
            .map_err(|e| io(parent, e))?;
        tmp.write_all(sealed).map_err(|e| io(parent, e))?;
        tmp.as_file().sync_all().map_err(|e| io(parent, e))?;
        tmp.persist(&path).map_err(|e| io(&path, e.error))?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|e| io(parent, e))?;
        Ok(())
    }

    fn read_blob(&self, id: &BlobId) -> Result<Vec<u8>> {
        let path = self.file_path(id);
        fs::read(&path).map_err(|e| map_read_error(&path, e))
    }

    fn seal(&self, id: &BlobId, plain: &[u8]) -> Result<Vec<u8>> {
        let key = self.key.as_ref().ok_or(BlobError::NoKey)?;
        if id.kid8() != key.key_id() {
            return Err(BlobError::NoKey);
        }
        let mut nonce = [0u8; 24];
        getrandom::getrandom(&mut nonce).map_err(|e| BlobError::Io(e.to_string()))?;
        let cipher = XChaCha20Poly1305::new((&key.enc).into());
        let aad = aad(id, plain.len() as u64);
        let body = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: &aad,
                },
            )
            .map_err(|_| BlobError::Corrupt)?;
        let mut out = Vec::with_capacity(HEADER_LEN + body.len());
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&(plain.len() as u64).to_le_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&body);
        Ok(out)
    }

    fn open_sealed(&self, id: &BlobId, sealed: &[u8]) -> Result<Vec<u8>> {
        let key = self.key.as_ref().ok_or(BlobError::NoKey)?;
        if id.kid8() != key.key_id() {
            return Err(BlobError::NoKey);
        }
        if sealed.len() < HEADER_LEN || sealed.len() as u64 > MAX_SEALED {
            return Err(BlobError::Corrupt);
        }
        if &sealed[..4] != MAGIC || sealed[4] != VERSION {
            return Err(BlobError::Corrupt);
        }
        let mut plain_size = [0u8; 8];
        plain_size.copy_from_slice(&sealed[5..13]);
        let plain_size = u64::from_le_bytes(plain_size);
        if plain_size > MAX_BLOB {
            return Err(BlobError::Corrupt);
        }
        let nonce = &sealed[13..37];
        let body = &sealed[HEADER_LEN..];
        let aad = aad(id, plain_size);
        let plain = XChaCha20Poly1305::new((&key.enc).into())
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: &aad,
                },
            )
            .map_err(|_| BlobError::Corrupt)?;
        if plain.len() as u64 != plain_size {
            return Err(BlobError::Corrupt);
        }
        Ok(plain)
    }

    fn clean_temp_files_at(&self, now: SystemTime) -> Result<()> {
        let cutoff = now.checked_sub(TMP_TTL).unwrap_or(now);
        let mut stack = vec![self.dir.clone()];
        while let Some(dir) = stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io(&dir, e)),
            };
            for entry in entries {
                let entry = entry.map_err(|e| io(&dir, e))?;
                let path = entry.path();
                let file_type = entry.file_type().map_err(|e| io(&path, e))?;
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|s| s.to_str()) != Some("tmp") {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .map_err(|e| io(&path, e))?;
                if modified <= cutoff {
                    match fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(io(&path, e)),
                    }
                }
            }
        }
        Ok(())
    }
}

fn aad(id: &BlobId, plain_size: u64) -> Vec<u8> {
    format!("terrarium-blob-v1|{}|{plain_size}", id.as_str()).into_bytes()
}

fn map_read_error(path: &Path, e: std::io::Error) -> BlobError {
    if e.kind() == std::io::ErrorKind::NotFound {
        BlobError::NotFound
    } else {
        io(path, e)
    }
}

fn io(path: &Path, e: std::io::Error) -> BlobError {
    BlobError::Io(format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = fs::Permissions::from_mode(0o700);
    fs::set_permissions(path, perms).map_err(|e| io(path, e))
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn hmac_sha256(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut k0 = [0u8; BLOCK];
    k0[..key.len()].copy_from_slice(key);
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k0[i];
        opad[i] ^= k0[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    let digest = outer.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_store() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = BlobStore::open(dir.path()).unwrap();
        store.set_key(BlobKey::new("space", &[7u8; 32]));
        (dir, store)
    }

    #[test]
    fn put_get_round_trip_and_dedupe() {
        let (_dir, mut store) = configured_store();
        let plain = b"hello encrypted blobs";
        let id = store.put(plain).unwrap();
        assert_eq!(store.get(&id).unwrap(), plain);
        assert_eq!(store.put(plain).unwrap(), id);
        assert_eq!(store.list(), vec![id]);
    }

    #[test]
    fn stored_file_does_not_contain_plaintext() {
        let (_dir, mut store) = configured_store();
        let plain = b"do not store this string in plaintext";
        let id = store.put(plain).unwrap();
        let sealed = store.get_sealed(&id).unwrap();
        assert!(!sealed.windows(plain.len()).any(|w| w == plain));
    }

    #[test]
    fn tampered_file_is_corrupt() {
        let (_dir, mut store) = configured_store();
        let id = store.put(b"blob body").unwrap();
        let path = store.file_path(&id);
        let mut sealed = fs::read(&path).unwrap();
        *sealed.last_mut().unwrap() ^= 1;
        fs::write(&path, sealed).unwrap();
        assert_eq!(store.get(&id), Err(BlobError::Corrupt));
    }

    #[test]
    fn put_sealed_rejects_wrong_id() {
        let (_dir, mut store) = configured_store();
        let id = store.put(b"blob body").unwrap();
        let sealed = store.get_sealed(&id).unwrap();
        let mut wrong = id.as_str().to_string();
        let replacement = if wrong.ends_with('0') { "1" } else { "0" };
        wrong.replace_range(wrong.len() - 1..wrong.len(), replacement);
        let wrong: BlobId = wrong.parse().unwrap();
        assert_eq!(store.put_sealed(&wrong, &sealed), Err(BlobError::Corrupt));
    }

    #[test]
    fn wrong_key_reports_no_key_or_corrupt_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = BlobStore::open(dir.path()).unwrap();
        writer.set_key(BlobKey::new("space", &[1u8; 32]));
        let id = writer.put(b"blob body").unwrap();

        let mut reader = BlobStore::open(dir.path()).unwrap();
        reader.set_key(BlobKey::new("space", &[2u8; 32]));
        let err = reader.get(&id).unwrap_err();
        assert!(matches!(err, BlobError::NoKey | BlobError::Corrupt));
    }

    #[test]
    fn too_large_is_rejected_before_writing() {
        let (_dir, mut store) = configured_store();
        let bytes = vec![9u8; MAX_BLOB as usize + 1];
        assert_eq!(store.put(&bytes), Err(BlobError::TooLarge));
        assert!(store.list().is_empty());
    }

    #[test]
    fn blob_id_parsing_rejects_paths_and_bad_lengths() {
        assert!("../x".parse::<BlobId>().is_err());
        assert!(
            "b1-ABCDEF12-0000000000000000000000000000000000000000000000000000000000000000"
                .parse::<BlobId>()
                .is_err()
        );
        assert!(
            "b1-12345678-00000000000000000000000000000000000000000000000000000000"
                .parse::<BlobId>()
                .is_err()
        );
    }

    #[test]
    fn leftover_temp_files_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("ab").join("cd");
        fs::create_dir_all(&nested).unwrap();
        let tmp = nested.join(".old.tmp");
        fs::write(&tmp, b"x").unwrap();

        let store = BlobStore::open(dir.path()).unwrap();
        let now = SystemTime::now()
            .checked_add(TMP_TTL + Duration::from_secs(1))
            .unwrap();
        store.clean_temp_files_at(now).unwrap();
        assert!(!tmp.exists());
    }

    #[test]
    fn debug_has_no_key_material() {
        let key = BlobKey::new("space", &[3u8; 32]);
        let debug = format!("{key:?}");
        assert!(debug.starts_with("BlobKey("));
        assert!(debug.contains(key.key_id()));
        assert!(!debug.contains("enc"));
        assert!(!debug.contains("id"));
    }
}
