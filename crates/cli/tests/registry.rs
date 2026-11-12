//! Publisher-facing commands, exercised with synthetic artifacts and disposable keys.
use std::process::Command;
use terrarium_runtime::{registry_index as index, Package};
const TERRA: &str = env!("CARGO_BIN_EXE_terra");
#[test]
fn structured_package_verification_and_explicit_separate_registry_signing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("dist")).unwrap();
    std::fs::write(
        root.join("manifest.json"),
        // Keep publisher verification independent of the runtime's growing fixture.
        serde_json::to_vec(&serde_json::json!({
            "id": "limits",
            "name": "Registry verification fixture",
            "version": "0.1.0",
            "api": "terrarium:plugin@0.1.0",
            "storage": { "version": 1, "tables": [] },
            "limits": { "fuel": 50000000, "memory_mb": 16, "timeout_ms": 5000 },
            "uses": ["tasks"]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(root.join("dist/plugin.wasm"), b"synthetic").unwrap();
    let (publisher, _) = terrarium_runtime::generate_signing_key().unwrap();
    let mut p = Package::from_dir(root).unwrap();
    p.sign(&publisher);
    let archive = root.join("fixture.terrapkg");
    let bytes = p.to_zip().unwrap();
    std::fs::write(&archive, &bytes).unwrap();
    let output = Command::new(TERRA)
        .args(["verify", archive.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let info: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(info["id"], "limits");
    assert_eq!(info["digest"], p.digest());
    assert_eq!(info["signer"], p.signer().unwrap());
    assert_eq!(
        info["capabilities"],
        serde_json::json!(["limits:50000000:16:5000", "uses:tasks"])
    );
    let (secret, key) = terrarium_runtime::generate_signing_key().unwrap();
    let config = index::Config {
        origin: "https://plugins.example.org".into(),
        keys: vec![key.clone()],
        revoked: vec![],
        minimum_sequence: 1,
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let catalog = serde_json::json!({"format":1,"registry":index::REGISTRY_ID,"sequence":1,"generated_ms":now,"expires_ms":now+index::DAY,"plugins":[]});
    let raw = format!("{}\n", serde_json::to_string_pretty(&catalog).unwrap()).into_bytes();
    let path = root.join("index.json");
    std::fs::write(&path, &raw).unwrap();
    let key_path = root.join("registry.key");
    let key_text: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&key_path, &key_text).unwrap();
    let missing = Command::new(TERRA)
        .args([
            "sign-index",
            path.to_str().unwrap(),
            "--origin",
            &config.origin,
        ])
        .env("TERRA_SIGNING_KEY", &key_path)
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(!root.join("index.sig.json").exists());
    assert!(!String::from_utf8_lossy(&missing.stderr).contains(&key_text));
    let signed = Command::new(TERRA)
        .args([
            "sign-index",
            path.to_str().unwrap(),
            "--origin",
            &config.origin,
            "--key",
            key_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        signed.status.success(),
        "{}",
        String::from_utf8_lossy(&signed.stderr)
    );
    assert!(signed.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&signed.stderr).contains(&key_text));
    let sig = std::fs::read(root.join("index.sig.json")).unwrap();
    assert!(index::verify(&raw, &sig, &config, &index::HighWater::default(), now).is_ok());
    // The registry key authenticates the catalog, never the package publisher.
    assert_ne!(Some(key.as_str()), p.signer());
}
