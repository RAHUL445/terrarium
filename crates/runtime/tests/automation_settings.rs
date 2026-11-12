//! Trusted Settings can re-arm this device without changing synced settings.
use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_runtime::{
    commands::{self, Caller, Kind, Scope},
    rules::Settings,
    services, HostConfig, PluginHost,
};

#[test]
fn start_is_local_space_scoped_and_not_a_client_command() {
    let root = TempDir::new().unwrap();
    let mut host = PluginHost::new(HostConfig {
        data_dir: root.path().to_owned(),
        spaces_dir: None,
        space: "personal".into(),
        device: "local".into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    let start = |host: &mut PluginHost, args: Value| {
        services::invoke(host, "rules_start", &args, true).unwrap()
    };
    assert_eq!(
        start(&mut host, json!({"space":"personal"})).unwrap()["armed"],
        false
    );
    for (device, paused, expected) in [
        ("remote", false, false),
        ("local", true, false),
        ("local", false, true),
    ] {
        host.rules_mut()
            .save_settings(
                "personal",
                &Settings {
                    primary_device: Some(device.into()),
                    paused,
                },
            )
            .unwrap();
        // A synced setting alone has no local authority.
        assert!(!host.automations_armed("personal").unwrap());
        let before = host.rules().settings("personal").unwrap();
        let result = start(&mut host, json!({"space":"personal"})).unwrap();
        assert_eq!(result["armed"], expected);
        assert_eq!(result["primary_device"], device);
        assert_eq!(result["paused"], paused);
        assert_eq!(host.rules().settings("personal").unwrap(), before);
    }
    assert!(start(&mut host, json!({"space":"closed"})).is_err());
    assert!(start(&mut host, json!({"space":"personal","id":"unexpected"})).is_err());
    for caller in [Caller::Client(Scope::Read), Caller::Client(Scope::Change)] {
        for name in ["rules_start", "rule.start"] {
            assert_eq!(
                commands::invoke(
                    &mut host,
                    caller,
                    Kind::Action,
                    name,
                    Some("personal"),
                    &json!({}),
                    true
                )
                .unwrap_err()
                .code,
                "not-found"
            );
        }
    }
}
