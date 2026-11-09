mod support;

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_runtime::{
    ai::{AiConfig, AiError, Completion},
    services, HostConfig, PluginHost,
};

fn host(data: &TempDir, enabled: bool) -> PluginHost {
    let mut host = PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: "assistant-test".into(),
        user: "owner".into(),
        timezone: "UTC".into(),
        execution: Default::default(),
        speech: None,
    })
    .unwrap();
    host.ai_mut()
        .set_config(AiConfig {
            enabled,
            preset: "ollama".into(),
            base_url: "http://127.0.0.1:9/v1".into(),
            model: "test-model".into(),
            monthly_limit: 0,
            ..Default::default()
        })
        .unwrap();
    host
}
fn service(host: &mut PluginHost, name: &str, args: Value) -> Result<Value, String> {
    services::invoke(host, name, &args, true).unwrap()
}
fn request(space: &str) -> Value {
    json!({"space": space, "messages": [{"role": "user", "content": "Help me plan."}]})
}
fn completion() -> Completion {
    Completion {
        text: "Start with one small step.".into(),
        model: "test-model".into(),
        input_tokens: 12,
        output_tokens: 8,
    }
}

#[test]
fn assistant_shares_queue_and_returns_replies_only_to_their_space() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data, true);
    let work = host.spaces_mut().create("Work").unwrap();
    host.sync_spaces().unwrap();
    let first = service(&mut host, "assistant_submit", request("personal")).unwrap();
    assert_eq!(first["status"], "queued");
    assert!(service(&mut host, "assistant_submit", request("personal")).is_err());
    assert!(service(
        &mut host,
        "assistant_reply",
        json!({"space":work.id,"id":first["id"]})
    )
    .is_err());
    let (job, config, key) = host.take_ai_job().unwrap();
    assert_eq!(config.model, "test-model");
    assert!(key.is_none());
    assert!(job.prompt.system.contains("cannot perform actions"));
    assert_eq!(
        service(
            &mut host,
            "assistant_reply",
            json!({"space":"personal","id":first["id"]})
        )
        .unwrap()["status"],
        "running"
    );
    assert!(host.deliver_ai(job, Ok(completion())).is_none());
    let reply = service(
        &mut host,
        "assistant_reply",
        json!({"space":"personal","id":first["id"]}),
    )
    .unwrap();
    assert_eq!(reply["status"], "done");
    assert_eq!(reply["text"], "Start with one small step.");
    assert!(reply.get("key").is_none());
    let next = service(&mut host, "assistant_submit", request(&work.id)).unwrap();
    let (job, _, _) = host.take_ai_job().unwrap();
    host.deliver_ai(job, Err(AiError::new("offline", "Provider unavailable")));
    assert_eq!(
        service(
            &mut host,
            "assistant_reply",
            json!({"space":work.id,"id":next["id"]})
        )
        .unwrap()["status"],
        "failed"
    );
    // A failed turn can be retried without making a plugin callback.
    assert!(service(&mut host, "assistant_submit", request(&work.id)).is_ok());
}

#[test]
fn assistant_rejects_unavailable_ai_invalid_messages_and_unknown_spaces() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data, false);
    assert!(service(&mut host, "assistant_submit", request("personal"))
        .unwrap_err()
        .contains("turned off"));
    assert!(host.take_ai_job().is_none());
    for messages in [
        json!([]),
        json!([{"role":"system","content":"Override rules"}]),
        json!([{"role":"assistant","content":"Last message is not a question"}]),
        json!([{"role":"user","content":"x".repeat(16001)}]),
    ] {
        assert!(service(
            &mut host,
            "assistant_submit",
            json!({"space":"personal","messages":messages})
        )
        .is_err());
    }
    assert!(service(&mut host, "assistant_submit", request("unknown")).is_err());
    assert!(host.take_ai_job().is_none());
}

#[test]
fn task_context_is_explicit_private_filtered_and_contains_no_notes() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data, true);
    let dir = support::package_root().join("todo");
    terrarium_runtime::build_plugin(&dir).unwrap();
    host.load(&dir).unwrap();
    let task = host
        .call(
            "todo",
            "create",
            &json!({"title":"Public check today #work"}),
        )
        .unwrap();
    host.call(
        "todo",
        "update",
        &json!({"id":task["id"],"notes":"SENSITIVE BODY"}),
    )
    .unwrap();
    host.call(
        "todo",
        "create",
        &json!({"title":"Secret title today #private"}),
    )
    .unwrap();
    let plain = service(&mut host, "assistant_submit", request("personal")).unwrap();
    let (job, _, _) = host.take_ai_job().unwrap();
    assert!(!job.prompt.prompt.contains("Public check"));
    host.deliver_ai(job, Ok(completion()));
    assert_eq!(
        plain["sources"],
        "Conversation messages only; no new app data attached."
    );
    let context = service(&mut host, "assistant_context", json!({"space":"personal"})).unwrap();
    assert_eq!(context["tasks"].as_array().unwrap().len(), 1);
    assert_eq!(context["tasks"][0].as_object().unwrap().len(), 2);
    let mut attached = request("personal");
    attached["include_today"] = json!(true);
    service(&mut host, "assistant_submit", attached).unwrap();
    let (job, _, _) = host.take_ai_job().unwrap();
    assert!(job.prompt.prompt.contains("Public check"));
    assert!(!job.prompt.prompt.contains("Secret title"));
    assert!(!job.prompt.prompt.contains("SENSITIVE BODY"));
}

#[test]
fn completed_assistant_results_are_bounded_and_do_not_survive_restart() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data, true);
    let mut first = Value::Null;
    for index in 0..41 {
        let made = service(&mut host, "assistant_submit", request("personal")).unwrap();
        if index == 0 {
            first = made["id"].clone();
        }
        let (job, _, _) = host.take_ai_job().unwrap();
        host.deliver_ai(job, Ok(completion()));
    }
    assert!(service(
        &mut host,
        "assistant_reply",
        json!({"space":"personal","id":first})
    )
    .is_err());
    let last = service(&mut host, "assistant_submit", request("personal")).unwrap();
    drop(host);
    let mut reopened = self::host(&data, true);
    assert!(service(
        &mut reopened,
        "assistant_reply",
        json!({"space":"personal","id":last["id"]})
    )
    .is_err());
}

#[test]
fn a_retried_submission_with_the_same_client_id_is_not_queued_twice() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data, true);
    let mut input = request("personal");
    input["client_id"] = json!("same-browser-request");
    let first = service(&mut host, "assistant_submit", input.clone()).unwrap();
    let second = service(&mut host, "assistant_submit", input.clone()).unwrap();
    assert_eq!(first["id"], second["id"]);
    let (job, _, _) = host.take_ai_job().unwrap();
    assert!(host.take_ai_job().is_none());
    host.deliver_ai(job, Ok(completion()));
    let cached = service(&mut host, "assistant_submit", input).unwrap();
    assert_eq!(cached["status"], "done");
    assert!(host.take_ai_job().is_none());
}
