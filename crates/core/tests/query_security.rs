use serde_json::json;
use terrarium_core::{PluginSchema, Store, StoreConfig, SystemTime};

fn store(root: &std::path::Path) -> Store {
    let schema: PluginSchema = serde_json::from_value(
        json!({"version":1,"tables":[{"name":"notes","columns":[{"name":"body","type":"text"}]}]}),
    )
    .unwrap();
    Store::open(
        root.join("notes.db"),
        StoreConfig {
            space: "personal".into(),
            plugin: "notes".into(),
            device: "test".into(),
            user: "owner".into(),
        },
        schema,
        Box::new(SystemTime),
    )
    .unwrap()
}

#[test]
fn sqlite_catalog_and_pragma_access_are_not_plugin_table_reads() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    for sql in [
        "SELECT name FROM sqlite_master",
        "PRAGMA table_info(notes)",
        "SELECT name FROM pragma_table_info('notes')",
    ] {
        assert!(store.query(sql, &[]).is_err(), "must reject: {sql}");
    }
}

#[test]
fn authorization_uses_sqlite_semantics_instead_of_substrings() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store(root.path());
    store
        .write(|w| {
            w.insert(
                "notes",
                &serde_json::from_value(json!({"body":"hello"})).unwrap(),
            )
        })
        .unwrap();
    assert_eq!(
        store.query("SELECT '_pp_meta' AS literal", &[]).unwrap()[0]["literal"],
        "_pp_meta"
    );
    assert_eq!(
        store
            .query_bounded("SELECT upper(body) AS body FROM NoTeS", &[])
            .unwrap()[0]["body"],
        "HELLO"
    );
    for sql in [
        "SELECT * FROM main.\"_pp_meta\"",
        "DELETE FROM notes",
        "UPDATE notes SET body = 'bad'",
        "CREATE TABLE evil(x)",
        "ATTACH ':memory:' AS other",
        "SELECT load_extension('anything')",
        "SELECT sqlite_compileoption_get(0)",
    ] {
        assert!(store.query_bounded(sql, &[]).is_err(), "must reject {sql}");
    }
    assert_eq!(
        store.query_bounded("SELECT body FROM notes", &[]).unwrap()[0]["body"],
        "hello"
    );
}

#[test]
fn plugin_queries_fail_on_row_byte_value_and_parameter_limits() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    for sql in [
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10001) SELECT x FROM n",
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000) SELECT printf('%01024d',x) AS body FROM n",
        "SELECT hex(zeroblob(600000)) AS huge",
    ] {
        assert!(store.query_bounded(sql, &[]).is_err(), "must be bounded: {sql}");
    }
    assert!(store
        .query_bounded("SELECT ?1", &[json!("x".repeat(1024 * 1024 + 1))])
        .is_err());
    assert!(store
        .query_bounded(&format!("SELECT 1 --{}", "x".repeat(64 * 1024)), &[])
        .is_err());
    assert!(store
        .check_query(&format!("SELECT 1 --{}", "x".repeat(64 * 1024)))
        .is_err());
    assert_eq!(
        store.query_bounded("SELECT 1 AS ok", &[]).unwrap()[0]["ok"],
        1
    );
}

#[test]
fn expensive_queries_interrupt_and_host_writes_and_reads_recover() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store(root.path());
    let expensive = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000000) SELECT sum(x) FROM n";
    assert!(store.query_bounded(expensive, &[]).is_err());
    let body = "x".repeat(1100 * 1024);
    store
        .write(|w| {
            w.insert(
                "notes",
                &serde_json::from_value(json!({"body":body})).unwrap(),
            )
        })
        .unwrap();
    assert!(store.query_bounded("SELECT body FROM notes", &[]).is_err());
    assert_eq!(
        store.query("SELECT body FROM notes", &[]).unwrap()[0]["body"]
            .as_str()
            .unwrap()
            .len(),
        1100 * 1024
    );
    assert_eq!(store.dump().unwrap()["notes"].len(), 1);
    assert!(
        !store.export_since(0).unwrap().0.changes.is_empty(),
        "host metadata access survives interruption"
    );
    store
        .write(|w| {
            w.insert(
                "notes",
                &serde_json::from_value(json!({"body":"after failure"})).unwrap(),
            )
        })
        .unwrap();
    assert_eq!(
        store
            .query_bounded("SELECT count(*) AS count FROM notes", &[])
            .unwrap()[0]["count"],
        2
    );
}

#[test]
fn reviewed_json_table_functions_preserve_reporting_queries() {
    let root = tempfile::tempdir().unwrap();
    let store = store(root.path());
    let rows = store
        .query_bounded(
            "SELECT json_extract(j.value, '$.label') AS label FROM json_each(?1) j ORDER BY j.key",
            &[json!("[{\"label\":\"Daily\"}]")],
        )
        .unwrap();
    assert_eq!(rows[0]["label"], "Daily");
    assert!(store
        .query_bounded("SELECT * FROM pragma_table_info('notes')", &[])
        .is_err());
}

#[test]
fn media_envelope_is_bounded_without_changing_default_query_limits() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store(root.path());
    store
        .write(|w| {
            w.insert(
                "notes",
                &serde_json::from_value(json!({"body":"a".repeat(1500 * 1024)})).unwrap(),
            )
        })
        .unwrap();
    assert!(store.query_bounded("SELECT body FROM notes", &[]).is_err());
    assert_eq!(
        store
            .query_bounded_media("SELECT body FROM notes", &[])
            .unwrap()[0]["body"]
            .as_str()
            .unwrap()
            .len(),
        1500 * 1024
    );
    assert!(store
        .query_bounded_media("SELECT hex(zeroblob(7000000))", &[])
        .is_err());
    assert!(store
        .query_bounded_media("SELECT * FROM _pp_meta", &[])
        .is_err());
}
