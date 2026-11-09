//! Where this device keeps its secret keys.
//!
//! - **macOS release builds** (or `TERRA_KEYCHAIN=1`): the login Keychain.
//!   Debug builds default to files, because an unsigned binary changes on
//!   every rebuild and macOS would ask to allow Keychain access each time.
//! - **Android**: files encrypted with an AES-256-GCM key that lives in the
//!   Android Keystore and never leaves it. App data is also excluded from
//!   backups (`allowBackup=false`).
//! - **Elsewhere**: files with owner-only permissions.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Secrets {
    dir: PathBuf,
    backend: Backend,
}

enum Backend {
    #[cfg(any(not(target_os = "android"), test))]
    Files,
    #[cfg(target_os = "macos")]
    Keychain { service: String },
    #[cfg(target_os = "android")]
    Keystore,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[cfg(target_os = "macos")]
fn keychain_service(scope: &str, profile_dir: Option<&Path>) -> String {
    match profile_dir {
        Some(dir) => format!(
            "io.github.rahul445.terrarium.profile/{}/{scope}",
            dir.display()
        ),
        None => format!("io.github.rahul445.terrarium/{scope}"),
    }
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

pub(crate) fn read_private(path: &Path) -> Result<Option<Vec<u8>>, String> {
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

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("Invalid secret name".into());
    }
    Ok(())
}

/// Copy and read back every key before recording the new backend or deleting
/// any source. Conflicting keys require explicit recovery rather than guessing.
#[cfg(any(target_os = "macos", test))]
fn migrate_files(source: &Secrets, target: &Secrets, marker: &Path) -> Result<(), String> {
    let mut copied = Vec::new();
    for name in ["identity", "space-key", "secrets-key", "drive"] {
        if let Some(bytes) = source.load(name)? {
            if target.load(name)?.is_some_and(|existing| existing != bytes) {
                return Err(format!(
                    "Secret migration conflict for {name}; original files were preserved"
                ));
            }
            copied.push((name, bytes));
        }
    }
    for (name, bytes) in &copied {
        if target.load(name)?.is_none() {
            target.save(name, bytes)?;
        }
        if target.load(name)?.as_deref() != Some(bytes.as_slice()) {
            return Err(format!(
                "Could not verify migrated {name}; original files were preserved"
            ));
        }
    }
    write_private(marker, b"keychain").map_err(err)?;
    for (name, bytes) in copied {
        if source.load(name)?.as_deref() != Some(bytes.as_slice()) {
            return Err(format!(
                "Legacy {name} changed during migration; it was preserved"
            ));
        }
        source.remove(name)?;
    }
    Ok(())
}

impl Secrets {
    /// `scope` distinguishes spaces, e.g. `personal`.
    ///
    /// The choice is recorded in `<dir>/secrets-backend` so switching
    /// between debug and release builds never loses keys. A release build
    /// on macOS moves keys from files into the Keychain once.
    pub fn open(dir: &Path, scope: &str) -> Result<Secrets, String> {
        let marker = dir.join("secrets-backend");
        let recorded = read_private(&marker)?
            .map(|bytes| String::from_utf8(bytes).map(|s| s.trim().to_string()))
            .transpose()
            .map_err(err)?;
        if recorded
            .as_deref()
            .is_some_and(|s| !matches!(s, "files" | "keychain" | "keystore"))
        {
            return Err("Unknown secret backend; storage was not changed".into());
        }
        #[cfg(not(target_os = "android"))]
        let files = Secrets {
            dir: dir.to_path_buf(),
            backend: Backend::Files,
        };
        let _ = scope;

        #[cfg(target_os = "macos")]
        {
            let profile_dir = std::env::var_os("TERRA_PROFILE_DIR").map(PathBuf::from);
            let keychain = || Secrets {
                dir: dir.to_path_buf(),
                backend: Backend::Keychain {
                    service: keychain_service(scope, profile_dir.as_deref()),
                },
            };
            let prefer_keychain =
                !cfg!(debug_assertions) || std::env::var_os("TERRA_KEYCHAIN").is_some();
            match recorded.as_deref() {
                Some("keychain") => {
                    let target = keychain();
                    migrate_files(&files, &target, &marker)?;
                    return Ok(target);
                }
                Some("keystore") => {
                    return Err("Android Keystore secrets cannot be opened on macOS".into())
                }
                Some("files") if !prefer_keychain => return Ok(files),
                _ if prefer_keychain => {
                    let target = keychain();
                    migrate_files(&files, &target, &marker)?;
                    return Ok(target);
                }
                _ => {}
            }
        }
        #[cfg(target_os = "android")]
        {
            if recorded.as_deref().is_some_and(|s| s != "keystore") {
                return Err("Existing secrets require an explicit migration to Android Keystore; storage was not changed".into());
            }
            write_private(&marker, b"keystore").map_err(err)?;
            Ok(Secrets {
                dir: dir.to_path_buf(),
                backend: Backend::Keystore,
            })
        }
        #[cfg(not(target_os = "android"))]
        {
            if recorded.as_deref().is_some_and(|s| s != "files") {
                return Err(
                    "Recorded secure storage backend is unavailable on this platform".into(),
                );
            }
            write_private(&marker, b"files").map_err(err)?;
            Ok(files)
        }
    }

    pub fn describe(&self) -> &'static str {
        match self.backend {
            #[cfg(any(not(target_os = "android"), test))]
            Backend::Files => "files (owner-only)",
            #[cfg(target_os = "macos")]
            Backend::Keychain { .. } => "macOS Keychain",
            #[cfg(target_os = "android")]
            Backend::Keystore => "Android Keystore",
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.secret"))
    }

    pub fn load(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        validate_name(name)?;
        match &self.backend {
            #[cfg(any(not(target_os = "android"), test))]
            Backend::Files => read_private(&self.file(name)),
            #[cfg(target_os = "macos")]
            Backend::Keychain { service } => {
                match keyring::Entry::new(service, name)
                    .map_err(err)?
                    .get_secret()
                {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(keyring::Error::NoEntry) => Ok(None),
                    Err(e) => Err(format!("Keychain: {e}")),
                }
            }
            #[cfg(target_os = "android")]
            Backend::Keystore => read_private(&self.file(name))?
                .map(|sealed| keystore::open(&sealed))
                .transpose(),
        }
    }

    pub fn save(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        validate_name(name)?;
        if bytes.len() > 1024 * 1024 - 64 {
            return Err("Secret is too large".into());
        }
        match &self.backend {
            #[cfg(any(not(target_os = "android"), test))]
            Backend::Files => write_private(&self.file(name), bytes).map_err(err),
            #[cfg(target_os = "macos")]
            Backend::Keychain { service } => keyring::Entry::new(service, name)
                .map_err(err)?
                .set_secret(bytes)
                .map_err(|e| format!("Keychain: {e}")),
            #[cfg(target_os = "android")]
            Backend::Keystore => {
                write_private(&self.file(name), &keystore::seal(bytes)?).map_err(err)
            }
        }
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        validate_name(name)?;
        match &self.backend {
            #[cfg(target_os = "macos")]
            Backend::Keychain { service } => match keyring::Entry::new(service, name)
                .map_err(err)?
                .delete_credential()
            {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(format!("Keychain: {e}")),
            },
            _ => match fs::remove_file(self.file(name)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(err(e)),
            },
        }
    }

    pub fn save_verified(&self, name: &str, secret: &[u8]) -> Result<(), String> {
        if let Some(existing) = self.load(name)? {
            if existing != secret {
                return Err(format!(
                    "Secret migration conflict for {name}; original data was preserved"
                ));
            }
        } else {
            self.save(name, secret)?;
        }
        if self.load(name)?.as_deref() != Some(secret) {
            return Err(format!("Could not verify migrated {name}"));
        }
        Ok(())
    }

    /// Moves a secret from an older plain file into this store, once.
    pub fn adopt(
        &self,
        name: &str,
        legacy: &Path,
        extract: impl Fn(&[u8]) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        let Some(bytes) = read_private(legacy)? else {
            return Ok(());
        };
        let secret =
            extract(&bytes).ok_or("Legacy secret is invalid; original file was preserved")?;
        self.save_verified(name, &secret)?;
        Ok(())
    }
}

#[cfg(target_os = "android")]
mod keystore {
    //! AES-256-GCM with a non-exportable key in the Android Keystore,
    //! called through JNI. Sealed format: 12-byte IV, then ciphertext+tag.

    use jni::objects::{JByteArray, JObject, JValue};
    use jni::JNIEnv;

    const ALIAS: &str = "terrarium-secrets";
    static KEY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<T>(f: impl FnOnce(&mut JNIEnv) -> jni::errors::Result<T>) -> Result<T, String> {
        let ctx = ndk_context::android_context();
        let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| e.to_string())?;
        let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
        let out = f(&mut env);
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return Err("Android Keystore call failed".into());
        }
        out.map_err(|e| format!("Android Keystore: {e}"))
    }

    fn key<'a>(env: &mut JNIEnv<'a>, create: bool) -> jni::errors::Result<JObject<'a>> {
        let _guard = KEY_LOCK
            .lock()
            .map_err(|_| jni::errors::Error::NullPtr("Keystore initialization lock"))?;
        let provider = env.new_string("AndroidKeyStore")?;
        let store = env
            .call_static_method(
                "java/security/KeyStore",
                "getInstance",
                "(Ljava/lang/String;)Ljava/security/KeyStore;",
                &[JValue::Object(&provider)],
            )?
            .l()?;
        env.call_method(
            &store,
            "load",
            "(Ljava/security/KeyStore$LoadStoreParameter;)V",
            &[JValue::Object(&JObject::null())],
        )?;
        let alias = env.new_string(ALIAS)?;
        let present = env
            .call_method(
                &store,
                "containsAlias",
                "(Ljava/lang/String;)Z",
                &[JValue::Object(&alias)],
            )?
            .z()?;
        if !present && !create {
            return Err(jni::errors::Error::NullPtr("existing Android Keystore key"));
        }
        if !present {
            // KeyGenParameterSpec.Builder(alias, ENCRYPT | DECRYPT)
            let builder = env.new_object(
                "android/security/keystore/KeyGenParameterSpec$Builder",
                "(Ljava/lang/String;I)V",
                &[JValue::Object(&alias), JValue::Int(1 | 2)],
            )?;
            let string_class = env.find_class("java/lang/String")?;
            let gcm = env.new_string("GCM")?;
            let modes = env.new_object_array(1, &string_class, &gcm)?;
            env.call_method(
                &builder,
                "setBlockModes",
                "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                &[JValue::Object(&modes)],
            )?;
            let none = env.new_string("NoPadding")?;
            let paddings = env.new_object_array(1, &string_class, &none)?;
            env.call_method(
                &builder,
                "setEncryptionPaddings",
                "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                &[JValue::Object(&paddings)],
            )?;
            env.call_method(
                &builder,
                "setKeySize",
                "(I)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
                &[JValue::Int(256)],
            )?;
            let spec = env
                .call_method(
                    &builder,
                    "build",
                    "()Landroid/security/keystore/KeyGenParameterSpec;",
                    &[],
                )?
                .l()?;
            let aes = env.new_string("AES")?;
            let generator = env
                .call_static_method(
                    "javax/crypto/KeyGenerator",
                    "getInstance",
                    "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
                    &[JValue::Object(&aes), JValue::Object(&provider)],
                )?
                .l()?;
            env.call_method(
                &generator,
                "init",
                "(Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Object(&spec)],
            )?;
            env.call_method(&generator, "generateKey", "()Ljavax/crypto/SecretKey;", &[])?;
        }
        env.call_method(
            &store,
            "getKey",
            "(Ljava/lang/String;[C)Ljava/security/Key;",
            &[JValue::Object(&alias), JValue::Object(&JObject::null())],
        )?
        .l()
    }

    fn cipher<'a>(env: &mut JNIEnv<'a>) -> jni::errors::Result<JObject<'a>> {
        let name = env.new_string("AES/GCM/NoPadding")?;
        env.call_static_method(
            "javax/crypto/Cipher",
            "getInstance",
            "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
            &[JValue::Object(&name)],
        )?
        .l()
    }

    pub fn seal(plain: &[u8]) -> Result<Vec<u8>, String> {
        with_env(|env| {
            let key = key(env, true)?;
            let cipher = cipher(env)?;
            env.call_method(
                &cipher,
                "init",
                "(ILjava/security/Key;)V",
                &[JValue::Int(1), JValue::Object(&key)],
            )?;
            let iv: JByteArray = env.call_method(&cipher, "getIV", "()[B", &[])?.l()?.into();
            let input = env.byte_array_from_slice(plain)?;
            let sealed: JByteArray = env
                .call_method(&cipher, "doFinal", "([B)[B", &[JValue::Object(&input)])?
                .l()?
                .into();
            let mut out = env.convert_byte_array(&iv)?;
            out.extend(env.convert_byte_array(&sealed)?);
            Ok(out)
        })
    }

    pub fn open(sealed: &[u8]) -> Result<Vec<u8>, String> {
        if sealed.len() < 12 + 16 {
            return Err("sealed secret is too short".into());
        }
        let (iv, body) = sealed.split_at(12);
        with_env(|env| {
            let key = key(env, false)?;
            let cipher = cipher(env)?;
            let iv = env.byte_array_from_slice(iv)?;
            let spec = env.new_object(
                "javax/crypto/spec/GCMParameterSpec",
                "(I[B)V",
                &[JValue::Int(128), JValue::Object(&iv)],
            )?;
            env.call_method(
                &cipher,
                "init",
                "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
                &[JValue::Int(2), JValue::Object(&key), JValue::Object(&spec)],
            )?;
            let input = env.byte_array_from_slice(body)?;
            let plain: JByteArray = env
                .call_method(&cipher, "doFinal", "([B)[B", &[JValue::Object(&input)])?
                .l()?
                .into();
            env.convert_byte_array(&plain)
        })
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn profiling_keychain_namespaces_are_separate_from_normal_and_each_other() {
        use super::keychain_service;
        use std::path::Path;
        let normal = keychain_service("personal", None);
        assert_eq!(normal, "io.github.rahul445.terrarium/personal");
        let first = keychain_service("personal", Some(Path::new("/tmp/profile-a")));
        assert_ne!(first, normal);
        assert_ne!(
            first,
            keychain_service("personal", Some(Path::new("/tmp/profile-b")))
        );
        assert_ne!(
            first,
            keychain_service("work", Some(Path::new("/tmp/profile-a")))
        );
    }

    /// Touches the login Keychain, so it only runs on request:
    /// `cargo test -p terrarium-app -- --ignored keychain`
    #[test]
    #[ignore]
    fn keychain_round_trip() {
        let service = format!("io.github.rahul445.terrarium/test-{}", std::process::id());
        let entry = keyring::Entry::new(&service, "probe").unwrap();
        entry.set_secret(b"secret bytes").unwrap();
        assert_eq!(entry.get_secret().unwrap(), b"secret bytes");
        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_secret(), Err(keyring::Error::NoEntry)));
    }
}

#[cfg(test)]
mod file_security_tests {
    use super::*;

    fn files(dir: &Path) -> Secrets {
        fs::create_dir_all(dir).unwrap();
        Secrets {
            dir: dir.to_path_buf(),
            backend: Backend::Files,
        }
    }

    #[test]
    fn private_writes_replace_whole_files_with_private_permissions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("key.secret");
        write_private(&path, b"old key").unwrap();
        write_private(&path, b"new key").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new key");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            1,
            "no temporary secrets remain"
        );
        let directory = root.path().join("directory");
        fs::create_dir(&directory).unwrap();
        assert!(write_private(&directory, b"key").is_err());
        assert!(directory.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn predictable_temp_and_destination_symlinks_do_not_overwrite_other_files() {
        let root = tempfile::tempdir().unwrap();
        let victim = root.path().join("victim");
        let path = root.path().join("identity.secret");
        fs::write(&victim, b"unchanged").unwrap();
        std::os::unix::fs::symlink(&victim, path.with_extension("tmp")).unwrap();
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        assert!(read_private(&path).is_err());
        write_private(&path, b"private key").unwrap();
        assert_eq!(fs::read(victim).unwrap(), b"unchanged");
        assert_eq!(fs::read(path).unwrap(), b"private key");
    }

    #[test]
    fn migration_copies_and_verifies_all_keys_including_drive_before_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let source = files(&root.path().join("legacy"));
        let target = files(&root.path().join("secure-synthetic"));
        let marker = source.dir.join("secrets-backend");
        for name in ["identity", "space-key", "secrets-key", "drive"] {
            source.save(name, name.as_bytes()).unwrap();
        }
        migrate_files(&source, &target, &marker).unwrap();
        for name in ["identity", "space-key", "secrets-key", "drive"] {
            assert_eq!(target.load(name).unwrap().unwrap(), name.as_bytes());
            assert!(source.load(name).unwrap().is_none());
        }
        assert_eq!(fs::read(marker).unwrap(), b"keychain");
    }

    #[test]
    fn migration_conflict_preserves_all_source_and_destination_keys_and_marker() {
        let root = tempfile::tempdir().unwrap();
        let source = files(&root.path().join("legacy"));
        let target = files(&root.path().join("secure-synthetic"));
        let marker = source.dir.join("secrets-backend");
        write_private(&marker, b"files").unwrap();
        source.save("identity", b"source identity").unwrap();
        source.save("space-key", b"source key").unwrap();
        target.save("space-key", b"different key").unwrap();
        assert!(migrate_files(&source, &target, &marker)
            .unwrap_err()
            .contains("conflict"));
        assert!(
            target.load("identity").unwrap().is_none(),
            "preflight before copying any key"
        );
        assert_eq!(target.load("space-key").unwrap().unwrap(), b"different key");
        assert_eq!(
            source.load("identity").unwrap().unwrap(),
            b"source identity"
        );
        assert_eq!(source.load("space-key").unwrap().unwrap(), b"source key");
        assert_eq!(fs::read(marker).unwrap(), b"files");
    }

    #[test]
    fn failed_marker_commit_keeps_verified_copies_and_every_original() {
        let root = tempfile::tempdir().unwrap();
        let source = files(&root.path().join("legacy"));
        let target = files(&root.path().join("secure-synthetic"));
        let marker = source.dir.join("marker-directory");
        fs::create_dir(&marker).unwrap();
        source.save("identity", b"identity").unwrap();
        assert!(migrate_files(&source, &target, &marker).is_err());
        assert_eq!(source.load("identity").unwrap().unwrap(), b"identity");
        assert_eq!(target.load("identity").unwrap().unwrap(), b"identity");
    }

    #[test]
    fn legacy_adoption_checks_conflicts_and_secret_paths_and_sizes_are_bounded() {
        let root = tempfile::tempdir().unwrap();
        let target = files(root.path());
        let legacy = root.path().join("identity.json");
        fs::write(&legacy, b"legacy key").unwrap();
        target.save("identity", b"different key").unwrap();
        assert!(target
            .adopt("identity", &legacy, |b| Some(b.to_vec()))
            .is_err());
        assert_eq!(fs::read(&legacy).unwrap(), b"legacy key");
        assert_eq!(target.load("identity").unwrap().unwrap(), b"different key");
        assert!(target.load("../key").is_err());
        assert!(target.save("../key", b"key").is_err());
        assert!(target.remove("../key").is_err());
        fs::File::create(target.file("oversized"))
            .unwrap()
            .set_len(1024 * 1024 + 1)
            .unwrap();
        assert!(target.load("oversized").is_err());
    }

    #[test]
    fn unknown_backend_marker_fails_without_rewriting_storage() {
        let root = tempfile::tempdir().unwrap();
        let marker = root.path().join("secrets-backend");
        fs::write(&marker, b"unknown-backend").unwrap();
        assert!(Secrets::open(root.path(), "synthetic-scope").is_err());
        assert_eq!(fs::read(marker).unwrap(), b"unknown-backend");
    }
}
