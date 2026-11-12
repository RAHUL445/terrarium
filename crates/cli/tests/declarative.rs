use std::{path::Path, process::Command};
const TERRA: &str = env!("CARGO_BIN_EXE_terra");
#[test]
fn declarative_build_check_pack_need_no_cargo_or_sdk() {
    let root = tempfile::tempdir().unwrap();
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../runtime/tests/fixtures/declarative");
    terrarium_runtime::Package::from_dir(&fixture)
        .unwrap()
        .write_to(root.path())
        .unwrap();
    for args in [vec!["check"], vec!["build"], vec!["pack"]] {
        let mut command = Command::new(TERRA);
        command.args(&args).arg(root.path());
        if args[0] == "pack" {
            command.arg("--unsigned");
        }
        let output = command
            .env("CARGO", "/does/not/exist")
            .env("TERRA_SDK", "/does/not/exist")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(root
        .path()
        .join("dist/reading-list-0.1.0.terrapkg")
        .exists());
    assert!(!root.path().join("dist/plugin.wasm").exists());
}

#[test]
fn declarative_new_ignores_inherited_sdk() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("reading-list");
    let output = Command::new(TERRA)
        .arg("new")
        .arg(&dir)
        .arg("--declarative")
        .env("TERRA_SDK", "/does/not/exist")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(&format!("terra check {}", dir.display())));
    assert!(stdout.contains(&format!("terra preview {}", dir.display())));
    let manifest = terrarium_runtime::Manifest::read(&dir).unwrap();
    assert_eq!(manifest.id, "reading-list");
    assert_eq!(manifest.name, "Reading List");
    let output = Command::new(TERRA).arg("check").arg(&dir).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
