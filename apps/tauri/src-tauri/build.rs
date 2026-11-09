fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bundled-plugins");
    println!("cargo:rerun-if-changed={}", root.display());
    let lock = root.parent().unwrap().join("plugins.lock.json");
    println!("cargo:rerun-if-changed={}", lock.display());
    let source = root.join("bundled.rs");
    assert!(
        source.is_file(),
        "Import a plugin bundle first: python3 scripts/import-plugins.py /path/to/release"
    );
    assert_eq!(
        std::fs::read(root.join("bundle.json")).expect("read imported bundle lock"),
        std::fs::read(lock).expect("read approved plugins.lock.json"),
        "Imported plugin bundle differs from the approved lock; run scripts/import-plugins.py"
    );
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::copy(source, out.join("bundled.rs")).expect("copy imported plugin table");
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "snapshot",
            "profile_startup",
            "add_item",
            "toggle_item",
            "bump_item",
            "delete_item",
            "restore_item",
            "simulate_conflict",
            "run_benchmark",
            "open_external",
            "plugins",
            "stage_install",
            "confirm_install",
            "cancel_install",
            "rollback_plugin",
            "set_plugin_removed",
            "trusted_signers",
            "service",
            "service_net",
            "list_themes",
            "install_theme",
            "remove_theme",
            "create_backup",
            "list_backups",
            "restore_backup",
            "view_template",
            "run_query",
            "run_action",
            "list_spaces",
            "create_space",
            "rename_space",
            "set_space_removed",
            "sync_status",
            "remove_sync_device",
            "start_pairing",
            "cancel_pairing",
            "join_space",
            "sync_now",
            "recovery_phrase",
            "connect_drive",
            "disconnect_drive",
        ]),
    ))
    .expect("build application permissions")
}
