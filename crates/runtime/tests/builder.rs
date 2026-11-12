use std::{fs, process::Command};

use terrarium_runtime::{build_plugin, componentize};

#[test]
fn build_uses_plugin_target_directory() {
    // Re-enter in a subprocess so environment overrides never affect other tests.
    const CHILD_DIR: &str = "TERRA_BUILDER_REGRESSION_DIR";
    if let Some(dir) = std::env::var_os(CHILD_DIR) {
        build_plugin(std::path::Path::new(&dir)).unwrap();
        return;
    }

    for use_env in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let plugin = temp.path().join("plugin");
        let redirected = temp.path().join("redirected");
        fs::create_dir_all(plugin.join("src")).unwrap();
        fs::create_dir(&redirected).unwrap();
        fs::write(
            plugin.join("Cargo.toml"),
            "[package]\nname = \"target-dir-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [workspace]\n[lib]\ncrate-type = [\"cdylib\"]\n",
        )
        .unwrap();
        fs::write(
            plugin.join("src/lib.rs"),
            "#[no_mangle]\npub extern \"C\" fn current_build() -> u32 { 42 }\n",
        )
        .unwrap();
        // A stale artifact must be replaced, rather than packaged after Cargo
        // silently sends the current build to a different target directory.
        let module = plugin.join("target/wasm32-unknown-unknown/release/target_dir_fixture.wasm");
        fs::create_dir_all(module.parent().unwrap()).unwrap();
        fs::write(&module, b"stale wasm").unwrap();

        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "build_uses_plugin_target_directory",
                "--nocapture",
            ])
            .current_dir(&plugin)
            .env(CHILD_DIR, &plugin);
        if use_env {
            child.env("CARGO_TARGET_DIR", &redirected);
        } else {
            child.env_remove("CARGO_TARGET_DIR");
            fs::create_dir(plugin.join(".cargo")).unwrap();
            fs::write(
                plugin.join(".cargo/config.toml"),
                "[build]\ntarget-dir = \"../redirected\"\n",
            )
            .unwrap();
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "build failed (environment override: {use_env}):\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let compiled = fs::read(&module).unwrap();
        assert_eq!(
            fs::read(plugin.join("dist/plugin.wasm")).unwrap(),
            componentize(&compiled).unwrap(),
        );
        assert_eq!(fs::read_dir(&redirected).unwrap().count(), 0);
    }
}
