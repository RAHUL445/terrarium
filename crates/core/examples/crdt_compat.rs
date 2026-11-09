//! Synthetic compatibility probe; never opens application data.
use loro::{ExportMode, LoroDoc};
use std::{env, fs, path::Path};

fn save(doc: &LoroDoc, root: &Path, name: &str) {
    fs::write(root.join(name), doc.export(ExportMode::Snapshot).unwrap()).unwrap();
}
fn load(root: &Path, name: &str) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.import(&fs::read(root.join(name)).unwrap()).unwrap();
    doc
}
fn main() {
    let args: Vec<_> = env::args().collect();
    let root = Path::new(&args[2]);
    match args[1].as_str() {
        "seed" => {
            fs::create_dir_all(root).unwrap();
            let doc = LoroDoc::new();
            doc.set_peer_id(11).unwrap();
            doc.get_text("body").insert(0, "Hello 🌍 नमस्ते\n").unwrap();
            doc.commit();
            save(&doc, root, "legacy.snapshot");
            let before = doc.oplog_vv();
            doc.get_text("body").insert(0, "offline ").unwrap();
            doc.commit();
            fs::write(
                root.join("legacy.update"),
                doc.export(ExportMode::updates(&before)).unwrap(),
            )
            .unwrap();
            save(&doc, root, "legacy-after.snapshot");
        }
        "forward" => {
            let doc = load(root, "legacy.snapshot");
            doc.set_peer_id(22).unwrap();
            let before = doc.oplog_vv();
            doc.get_text("body").insert(0, "new ").unwrap();
            doc.commit();
            fs::write(
                root.join("new.update"),
                doc.export(ExportMode::updates(&before)).unwrap(),
            )
            .unwrap();
            save(&doc, root, "new.snapshot");
            doc.import(&fs::read(root.join("legacy.update")).unwrap())
                .unwrap();
            save(&doc, root, "new-merged.snapshot");
            fs::write(root.join("merged.txt"), doc.get_text("body").to_string()).unwrap();
            // Corruption must fail without rewriting the source fixture.
            assert!(LoroDoc::new().import(b"invalid snapshot").is_err());
        }
        "backward" => {
            let doc = load(root, "legacy-after.snapshot");
            doc.import(&fs::read(root.join("new.update")).unwrap())
                .unwrap();
            let expected = fs::read_to_string(root.join("merged.txt")).unwrap();
            assert_eq!(doc.get_text("body").to_string(), expected);
            assert_eq!(
                load(root, "new-merged.snapshot")
                    .get_text("body")
                    .to_string(),
                expected
            );
            assert!(load(root, "new.snapshot")
                .get_text("body")
                .to_string()
                .starts_with("new "));
        }
        _ => panic!("seed|forward|backward DIR"),
    }
}
