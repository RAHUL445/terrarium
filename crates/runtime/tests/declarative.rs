use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::{
    commands::{self, Caller, InvokeOptions, Kind as CommandKind, Scope},
    HostConfig, Manifest, Package, PluginHost, RuntimeError, ViewKind, DEFAULT_SPACE,
};
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/declarative")
}
fn manifest() -> Value {
    serde_json::from_slice(&std::fs::read(fixture().join("manifest.json")).unwrap()).unwrap()
}
fn parsed(v: &Value) -> terrarium_runtime::Result<Manifest> {
    Manifest::parse(&serde_json::to_vec(v).unwrap())
}
fn host(root: &Path, name: &str) -> PluginHost {
    let mut h = PluginHost::new(HostConfig {
        data_dir: root.join(name),
        spaces_dir: None,
        space: "test".into(),
        device: name.into(),
        user: "test".into(),
        timezone: "Asia/Kolkata".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    h.load(&fixture()).unwrap();
    h
}
fn rows(h: &PluginHost, query: &str) -> Vec<Map<String, Value>> {
    h.query("reading-list", query, &Map::new()).unwrap()
}
fn call(h: &mut PluginHost, op: &str, args: Value) -> Value {
    h.call("reading-list", op, &args).unwrap()
}
fn code(r: terrarium_runtime::Result<Value>, expected: &str) {
    match r.unwrap_err() {
        RuntimeError::Plugin { code, .. } => assert_eq!(code, expected),
        e => panic!("{e}"),
    }
}
fn edited(root: &Path, edit: impl FnOnce(&mut Value)) -> PathBuf {
    let dir = root.join("edited");
    Package::from_dir(&fixture())
        .unwrap()
        .write_to(&dir)
        .unwrap();
    let mut m = manifest();
    edit(&mut m);
    std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    dir
}
#[test]
fn create_update_toggle_delete_restore() {
    let tmp = TempDir::new().unwrap();
    let mut h = host(tmp.path(), "a");
    assert!(h.template("reading-list", ViewKind::Page, "main").is_ok());
    let id = call(&mut h, "create", json!({"title":"{{args.title}}"}))["id"].clone();
    assert_eq!(rows(&h, "open")[0]["title"], "{{args.title}}");
    assert_eq!(
        call(&mut h, "update", json!({"id":id,"title":"Read"})),
        Value::Null
    );
    call(&mut h, "toggle", json!({"id":id,"done":false}));
    assert_eq!(rows(&h, "open").len(), 1);
    call(&mut h, "toggle", json!({"id":id,"done":true}));
    assert!(rows(&h, "open").is_empty());
    assert_eq!(rows(&h, "done").len(), 1);
    call(&mut h, "delete", json!({"id":id}));
    assert_eq!(rows(&h, "trash").len(), 1);
    code(
        h.call("reading-list", "delete", &json!({"id":id})),
        "in-trash",
    );
    code(
        h.call("reading-list", "toggle", &json!({"id":id,"done":false})),
        "in-trash",
    );
    call(&mut h, "restore", json!({"id":id}));
    assert_eq!(rows(&h, "done").len(), 1);
    code(
        h.call("reading-list", "restore", &json!({"id":id})),
        "not-found",
    );
    code(
        h.call("reading-list", "delete", &json!({"id":"missing"})),
        "not-found",
    );
}
#[test]
fn missing_or_wrong_args_are_rejected_for_every_caller() {
    let tmp = TempDir::new().unwrap();
    let mut h = host(tmp.path(), "a");
    for args in [
        Value::Null,
        json!({}),
        json!({"title":null}),
        json!({"title":4}),
        json!({"title":"a","extra":1}),
        json!([]),
    ] {
        code(h.call("reading-list", "create", &args), "invalid");
        for caller in [Caller::App, Caller::Client(Scope::Change)] {
            assert_eq!(
                commands::invoke(
                    &mut h,
                    caller,
                    CommandKind::Action,
                    "reading-list.create",
                    None,
                    &args,
                    true
                )
                .unwrap_err()
                .code,
                "invalid"
            );
        }
    }
    let id = call(&mut h, "create", json!({"title":""}))["id"].clone();
    for done in [json!(0), json!("false"), Value::Null] {
        code(
            h.call("reading-list", "toggle", &json!({"id":id,"done":done})),
            "invalid",
        );
    }
    assert_eq!(
        commands::invoke(
            &mut h,
            Caller::Client(Scope::Read),
            CommandKind::Action,
            "reading-list.create",
            None,
            &json!({"title":"x"}),
            true
        )
        .unwrap_err()
        .code,
        "forbidden"
    );
    assert_eq!(
        commands::invoke_with(
            &mut h,
            Caller::Client(Scope::Change),
            CommandKind::Action,
            "reading-list.delete",
            None,
            &json!({"id":id}),
            true,
            InvokeOptions {
                dry_run: false,
                confirmed: false
            }
        )
        .unwrap_err()
        .code,
        "needs-confirmation"
    );
}
#[test]
fn dry_run_keeps_nothing_and_traces_changes() {
    let tmp = TempDir::new().unwrap();
    let mut h = host(tmp.path(), "a");
    let before = h
        .store("reading-list")
        .unwrap()
        .export_since(0)
        .unwrap()
        .0
        .changes
        .len();
    let (v, trace) = h
        .call_traced(
            DEFAULT_SPACE,
            "reading-list",
            "create",
            &json!({"title":"dry"}),
            true,
        )
        .unwrap();
    assert!(v["id"].is_string());
    assert!(!trace.is_empty());
    assert!(rows(&h, "open").is_empty());
    assert_eq!(
        h.store("reading-list")
            .unwrap()
            .export_since(0)
            .unwrap()
            .0
            .changes
            .len(),
        before
    );
    let id = call(&mut h, "create", json!({"title":"kept"}))["id"].clone();
    h.call_traced(
        DEFAULT_SPACE,
        "reading-list",
        "delete",
        &json!({"id":id}),
        true,
    )
    .unwrap();
    assert!(rows(&h, "trash").is_empty());
    assert_eq!(rows(&h, "open").len(), 1);
}
#[test]
fn changes_sync_like_wasm_plugins() {
    let tmp = TempDir::new().unwrap();
    let mut a = host(tmp.path(), "a");
    let mut b = host(tmp.path(), "b");
    let other = b.spaces_mut().create("Other").unwrap().id;
    b.sync_spaces().unwrap();
    let id = call(&mut a, "create", json!({"title":"Synced"}))["id"].clone();
    for op in ["toggle", "delete", "restore"] {
        call(
            &mut a,
            op,
            if op == "toggle" {
                json!({"id":id,"done":true})
            } else {
                json!({"id":id})
            },
        );
        let batch = a.store("reading-list").unwrap().export_since(0).unwrap().0;
        b.store_mut("reading-list").unwrap().import(&batch).unwrap();
        assert_eq!(rows(&a, "done"), rows(&b, "done"));
        assert_eq!(rows(&a, "trash"), rows(&b, "trash"));
        assert!(b
            .query_in(&other, "reading-list", "done", &Map::new())
            .unwrap()
            .is_empty());
    }
}
#[test]
fn wasm_manifest_with_do_is_rejected() {
    let mut v = manifest();
    v["kind"] = json!("wasm");
    v["api"] = json!(terrarium_runtime::API_VERSION);
    assert!(parsed(&v).is_err());
    v["operations"]["create"]["do"] = Value::Null;
    assert!(parsed(&v).is_err());
}
#[test]
fn manifest_rejects_invalid_contracts() {
    let cases = [
        ("kind", Value::Null),
        ("kind", json!("future")),
        ("api", json!(terrarium_runtime::API_VERSION)),
        ("permissions", json!(["ai"])),
        ("uses", json!(["other"])),
        ("provides", json!({"x":{"operations":[]}})),
        (
            "jobs",
            json!({"x":{"operation":"create","schedule":{"every_minutes":5}}}),
        ),
        ("reminders", json!({"query":"open"})),
    ];
    for (k, value) in cases {
        let mut v = manifest();
        v[k] = value;
        assert!(parsed(&v).is_err(), "{k}");
    }
    for body in [
        Value::Null,
        json!([]),
        json!([{"insert":"items","values":{}}]),
        json!([{"insert":"other","values":{"title":"x"}}]),
        json!([{"insert":"items","values":{"id":"x"}}]),
        json!([{"insert":"items","values":{"title":"prefix {{args.title}}"}}]),
        json!([{"insert":"items","values":{"title":"{{args.nope}}"}}]),
        json!([{"insert":"items","values":{"done":"{{args.title}}"}}]),
        json!([{"insert":"items","values":{"title":{}}}]),
        json!([{"insert":"items","values":{"title":"x"},"extra":true}]),
        json!([{"delete":"items","row":"x","update":"items","values":{"title":"x"}}]),
    ] {
        let mut v = manifest();
        v["operations"]["create"]["do"] = body;
        assert!(parsed(&v).is_err(), "{v}");
    }
    for key in ["params", "do"] {
        let mut v = manifest();
        v["operations"]["create"]
            .as_object_mut()
            .unwrap()
            .remove(key);
        assert!(parsed(&v).is_err());
    }
    for effect in ["read", "write"] {
        let mut v = manifest();
        v["operations"]["delete"]["effect"] = json!(effect);
        assert!(parsed(&v).is_err());
    }
    let mut v = manifest();
    v["operations"]["update"]["do"] = json!(vec![
        json!({"update":"items","row":"{{args.id}}","values":{"title":"x"}});
        16
    ]);
    assert!(parsed(&v).is_ok());
    v["operations"]["update"]["do"]
        .as_array_mut()
        .unwrap()
        .push(json!({"delete":"items","row":"x"}));
    assert!(parsed(&v).is_err());
}
#[test]
fn optional_values_and_atomic_multistep_counter() {
    let tmp = TempDir::new().unwrap();
    let dir = edited(tmp.path(), |m| {
        m["storage"]["version"] = json!(2);
        m["storage"]["tables"][0]["columns"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({"name":"visits","type":"integer","rule":"counter"}),
                json!({"name":"day","type":"text"}),
            ]);
        m["operations"]["visit"] = json!({"params":{"id":{"type":"id","required":true},"other":{"type":"id","required":true}},"do":[{"increment":"items","row":"{{args.id}}","column":"visits","delta":1},{"update":"items","row":"{{args.other}}","values":{"day":"{{$today}}","created_ms":"{{$now_ms}}"}}]});
        m["operations"]["optional"] = json!({"params":{"id":{"type":"id","required":true},"title":{"type":"string"}},"do":[{"update":"items","row":"{{args.id}}","values":{"title":"{{args.title}}"}}]});
    });
    let mut h = host(tmp.path(), "a");
    h.load(&dir).unwrap();
    let id = call(&mut h, "create", json!({"title":"a"}))["id"].clone();
    let before = h
        .store("reading-list")
        .unwrap()
        .export_since(0)
        .unwrap()
        .0
        .changes
        .len();
    code(
        h.call("reading-list", "visit", &json!({"id":id,"other":"missing"})),
        "not-found",
    );
    assert_eq!(
        h.store("reading-list")
            .unwrap()
            .export_since(0)
            .unwrap()
            .0
            .changes
            .len(),
        before
    );
    assert_eq!(
        h.store("reading-list")
            .unwrap()
            .query("SELECT visits FROM items", &[])
            .unwrap()[0]["visits"],
        0
    );
    call(&mut h, "visit", json!({"id":id,"other":id}));
    let r = &h
        .store("reading-list")
        .unwrap()
        .query("SELECT visits, day, created_ms FROM items", &[])
        .unwrap()[0];
    assert_eq!(r["visits"], 1);
    assert!(r["day"].as_str().unwrap().len() == 10);
    assert!(r["created_ms"].as_i64().unwrap() > 0);
    code(
        h.call("reading-list", "optional", &json!({"id":id})),
        "invalid",
    );
    call(&mut h, "optional", json!({"id":id,"title":null}));
    assert!(rows(&h, "open")[0]["title"].is_null());
}
#[test]
fn input_budget_is_enforced_and_store_is_returned() {
    let tmp = TempDir::new().unwrap();
    let mut h = host(tmp.path(), "a");
    code(
        h.call(
            "reading-list",
            "create",
            &json!({"title":"x".repeat(12*1024*1024)}),
        ),
        "quota",
    );
    call(&mut h, "create", json!({"title":"ok"}));
}

#[test]
fn cumulative_writes_roll_back_and_return_the_store() {
    let tmp = TempDir::new().unwrap();
    let dir = edited(tmp.path(), |m| {
        m["limits"] = json!({"timeout_ms":10000});
        m["operations"]["update"]["do"] = json!(vec![
            json!({"update":"items","row":"{{args.id}}","values":{"title":"{{args.title}}"}});
            5
        ]);
    });
    let mut h = host(tmp.path(), "a");
    h.load(&dir).unwrap();
    let id = call(&mut h, "create", json!({"title":"original"}))["id"].clone();
    let before = h
        .store("reading-list")
        .unwrap()
        .export_since(0)
        .unwrap()
        .0
        .changes
        .len();
    code(
        h.call(
            "reading-list",
            "update",
            &json!({"id":id,"title":"x".repeat(7*1024*1024)}),
        ),
        "quota",
    );
    assert_eq!(rows(&h, "open")[0]["title"], "original");
    assert_eq!(
        h.store("reading-list")
            .unwrap()
            .export_since(0)
            .unwrap()
            .0
            .changes
            .len(),
        before
    );
    call(&mut h, "update", json!({"id":id,"title":"recovered"}));
}
#[test]
fn synced_external_schedules_cannot_dispatch_declarative_operations() {
    use terrarium_runtime::scheduler::{Repeat, ScheduleInput};
    let tmp = TempDir::new().unwrap();
    let mut h = host(tmp.path(), "a");
    let id = h
        .scheduler_mut()
        .set(
            DEFAULT_SPACE,
            "reading-list",
            "external",
            &ScheduleInput {
                name: "External".into(),
                repeat: Repeat::Minutes { minutes: 5 },
                tz: None,
                run_on: Default::default(),
                operation: "create".into(),
                args: json!({"title":"background"}),
                enabled: true,
            },
            "user",
        )
        .unwrap();
    // Simulates a schedule imported from a peer/low-level store. Host entry
    // points refuse edits, and dispatch also refuses an already queued run.
    for name in [
        "schedule_set_enabled",
        "schedule_set_repeat",
        "schedule_run_now",
    ] {
        assert!(terrarium_runtime::services::invoke(
            &mut h,
            name,
            &json!({"id":id,"enabled":true,"repeat":{"minutes":{"minutes":5}}}),
            true
        )
        .unwrap()
        .is_err());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    h.scheduler_mut().run_now(&id, now).unwrap();
    let finished = h.run_due(now, true);
    assert!(!finished.is_empty());
    assert!(rows(&h, "open").is_empty());
}
#[test]
fn default_wasm_manifest_remains_compatible_and_new_api_errors_explain_upgrade() {
    let old = Manifest::parse(include_bytes!("fixtures/limits/manifest.json")).unwrap();
    assert_eq!(old.kind, terrarium_runtime::Kind::Wasm);
    assert!(serde_json::to_value(old).unwrap().get("kind").is_none());
    let mut v = manifest();
    v["api"] = json!("terrarium:declarative@9.0.0");
    let error = parsed(&v).unwrap_err().to_string();
    assert!(error.contains("Update Terrarium on each device"));
    assert!(error.contains("terrarium:declarative@9.0.0"));
}
