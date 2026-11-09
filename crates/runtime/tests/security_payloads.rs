use serde_json::json;
use terrarium_runtime::{build_plugin, HostConfig, PluginHost};

#[test]
fn hostile_success_reply_and_write_budget_roll_back_the_operation() {
    let scratch = tempfile::tempdir().unwrap();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/limits");
    build_plugin(&source).unwrap();
    let package = scratch.path().join("package");
    std::fs::create_dir_all(package.join("dist")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(source.join("manifest.json")).unwrap()).unwrap();
    manifest["limits"] = json!({"fuel":2_000_000_000u64,"memory_mb":128,"timeout_ms":10000});
    std::fs::write(
        package.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::copy(
        source.join("dist/plugin.wasm"),
        package.join("dist/plugin.wasm"),
    )
    .unwrap();
    let mut host = PluginHost::new(HostConfig {
        data_dir: scratch.path().join("data"),
        spaces_dir: None,
        space: "test".into(),
        device: "test-device".into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    host.load(&package).unwrap();
    for (operation, expected) in [("oversized_reply", "result"), ("oversized_writes", "write")] {
        let error = host
            .call("limits", operation, &json!({}))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(expected),
            "wrong boundary for {operation}: {error}"
        );
        assert!(
            host.query("limits", "all", &Default::default())
                .unwrap()
                .is_empty(),
            "partial writes survived {operation}"
        );
        assert!(host.call("limits", "ok", &json!({})).is_ok());
    }
}
