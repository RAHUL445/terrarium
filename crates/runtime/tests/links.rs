//! Host relation commands, permissions and `_links` sync wiring.
use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::{
    commands::{self, Caller, InvokeOptions, Kind, Scope},
    HostConfig, PluginHost, LINKS_STORE_ID,
};
const A: &str = "todo/tasks/t1";
const B: &str = "notes/notes/n1";
fn host(root: &TempDir, device: &str) -> PluginHost {
    PluginHost::new(HostConfig {
        data_dir: root.path().join(device).join("personal"),
        spaces_dir: Some(root.path().join(device).join("spaces")),
        space: "personal".into(),
        device: device.into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap()
}
fn call(host: &mut PluginHost, kind: Kind, id: &str, args: Value) -> Value {
    commands::invoke(host, Caller::App, kind, id, None, &args, true).unwrap()
}
#[test]
fn link_commands_round_trip() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    let args = json!({"source":A,"target":B});
    let added = call(&mut host, Kind::Action, "link.add", args.clone());
    assert_eq!(added["kind"], "related");
    assert_eq!(
        call(&mut host, Kind::Action, "link.add", args)["id"],
        added["id"]
    );
    for (item, other, direction) in [(A, B, "out"), (B, A, "in")] {
        let links = call(&mut host, Kind::Query, "link.list", json!({"ref":item}));
        assert_eq!(links.as_array().unwrap().len(), 1);
        assert_eq!(links[0]["id"], added["id"]);
        assert_eq!(links[0]["source"], A);
        assert_eq!(links[0]["target"], B);
        assert_eq!(links[0]["kind"], "related");
        assert_eq!(links[0]["created_ms"], added["created_ms"]);
        assert_eq!(links[0]["other"], other);
        assert_eq!(links[0]["direction"], direction);
    }
    assert_eq!(
        call(
            &mut host,
            Kind::Action,
            "link.remove",
            json!({"source":B,"target":A})
        ),
        json!({"removed":1})
    );
    assert_eq!(
        call(&mut host, Kind::Query, "link.list", json!({"ref":A})),
        json!([])
    );
    assert!(commands::HOST_NAMESPACES.contains(&"link"));
}
#[test]
fn links_sync_with_other_devices() {
    let root = TempDir::new().unwrap();
    let mut a = host(&root, "a");
    let mut b = host(&root, "b");
    assert!(a.plugin_ids().contains(&LINKS_STORE_ID.to_string()));
    let added = call(
        &mut a,
        Kind::Action,
        "link.add",
        json!({"source":A,"target":B}),
    );
    let (batch, _) = a
        .store_mut(LINKS_STORE_ID)
        .unwrap()
        .export_since(0)
        .unwrap();
    b.store_mut(LINKS_STORE_ID).unwrap().import(&batch).unwrap();
    let links = call(&mut b, Kind::Query, "link.list", json!({"ref":A}));
    assert_eq!(links[0]["id"], added["id"]);
}
#[test]
fn outside_callers_need_confirmation_to_remove() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    let args = json!({"source":A,"target":B});
    assert_eq!(
        commands::invoke(
            &mut host,
            Caller::Client(Scope::Read),
            Kind::Action,
            "link.add",
            None,
            &args,
            true
        )
        .unwrap_err()
        .code,
        "forbidden"
    );
    commands::invoke(
        &mut host,
        Caller::Client(Scope::Change),
        Kind::Action,
        "link.add",
        None,
        &args,
        true,
    )
    .unwrap();
    assert_eq!(
        commands::invoke_with(
            &mut host,
            Caller::Client(Scope::Change),
            Kind::Action,
            "link.remove",
            None,
            &args,
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
    let links = commands::invoke(
        &mut host,
        Caller::Client(Scope::Read),
        Kind::Query,
        "link.list",
        None,
        &json!({"ref":A}),
        true,
    )
    .unwrap();
    assert_eq!(links.as_array().unwrap().len(), 1);
    commands::invoke_with(
        &mut host,
        Caller::Client(Scope::Change),
        Kind::Action,
        "link.remove",
        None,
        &args,
        true,
        InvokeOptions {
            dry_run: false,
            confirmed: true,
        },
    )
    .unwrap();
    assert_eq!(
        call(&mut host, Kind::Query, "link.list", json!({"ref":A})),
        json!([])
    );
}
#[test]
fn commands_respect_spaces_and_validate_input() {
    let root = TempDir::new().unwrap();
    let mut host = host(&root, "a");
    let work = host.spaces_mut().create("Work").unwrap();
    host.sync_spaces().unwrap();
    let args = json!({"source":A,"target":B});
    call(&mut host, Kind::Action, "link.add", args.clone());
    let other = commands::invoke(
        &mut host,
        Caller::App,
        Kind::Query,
        "link.list",
        Some(&work.id),
        &json!({"ref":A}),
        true,
    )
    .unwrap();
    assert_eq!(other, json!([]));
    commands::invoke(
        &mut host,
        Caller::App,
        Kind::Action,
        "link.remove",
        Some(&work.id),
        &args,
        true,
    )
    .unwrap();
    assert_eq!(
        call(&mut host, Kind::Query, "link.list", json!({"ref":A}))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    for bad in [
        json!({"source":"bad","target":B}),
        json!({"source":A,"target":A}),
        json!({"source":A,"target":B,"kind":"parent"}),
        json!({"source":A,"target":B,"kind":42}),
    ] {
        assert!(commands::invoke(
            &mut host,
            Caller::App,
            Kind::Action,
            "link.add",
            None,
            &bad,
            true
        )
        .is_err());
    }
}
