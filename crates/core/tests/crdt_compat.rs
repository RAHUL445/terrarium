//! Fixtures produced by the unmodified registry Loro 1.17.1 before the backport.
use loro::{ExportMode, LoroDoc};
const SNAPSHOT: &[u8] = include_bytes!("fixtures/loro-1.17.1/legacy.snapshot");
const UPDATE: &[u8] = include_bytes!("fixtures/loro-1.17.1/legacy.update");
const EXPECTED: &str = "Hello 🌍 नमस्ते\n";

#[test]
fn legacy_snapshot_update_and_new_snapshot_reopen() {
    let doc = LoroDoc::new();
    doc.import(SNAPSHOT).unwrap();
    assert_eq!(doc.get_text("body").to_string(), EXPECTED);
    doc.import(UPDATE).unwrap();
    doc.import(UPDATE).unwrap(); // Retries are idempotent.
    assert_eq!(
        doc.get_text("body").to_string(),
        format!("offline {EXPECTED}")
    );
    let reopened = LoroDoc::new();
    reopened
        .import(&doc.export(ExportMode::Snapshot).unwrap())
        .unwrap();
    assert_eq!(
        reopened.get_text("body").to_string(),
        doc.get_text("body").to_string()
    );
}

#[test]
fn legacy_offline_edit_and_new_unicode_edit_converge_in_both_orders() {
    let left = LoroDoc::new();
    left.import(SNAPSHOT).unwrap();
    left.set_peer_id(22).unwrap();
    let before = left.oplog_vv();
    left.get_text("body").insert(0, "new 🦀 ").unwrap();
    left.commit();
    let update = left.export(ExportMode::updates(&before)).unwrap();
    left.import(UPDATE).unwrap();
    let right = LoroDoc::new();
    right.import(SNAPSHOT).unwrap();
    right.import(UPDATE).unwrap();
    right.import(&update).unwrap();
    assert_eq!(
        left.get_text("body").to_string(),
        right.get_text("body").to_string()
    );
    assert!(left.get_text("body").to_string().contains(EXPECTED));
}

#[test]
fn invalid_import_does_not_damage_existing_text() {
    let doc = LoroDoc::new();
    doc.import(SNAPSHOT).unwrap();
    assert!(doc.import(b"invalid snapshot").is_err());
    assert_eq!(doc.get_text("body").to_string(), EXPECTED);
}
