//! Test fixture: operations that misbehave on purpose.

use serde_json::{json, Map, Value};
use terrarium_guest::{storage, Args, Error, Plugin, Result};

struct Limits;

impl Plugin for Limits {
    fn handle(operation: &str, args: Args) -> Result<Value> {
        match operation {
            "note" => {
                let id = storage::insert(
                    "notes",
                    &json!({"note": args.str("text")?})
                        .as_object()
                        .unwrap()
                        .clone(),
                )?;
                Ok(json!({"id": id}))
            }
            "mark" => {
                storage::update(
                    "notes",
                    args.str("id")?,
                    &json!({"note": args.str("text")?})
                        .as_object()
                        .unwrap()
                        .clone(),
                )?;
                Ok(Value::Null)
            }
            "mark_many" => {
                let ids = args
                    .get("ids")
                    .and_then(Value::as_array)
                    .ok_or_else(|| Error::invalid("`ids` must be a list"))?;
                for id in ids {
                    let id = id
                        .as_str()
                        .ok_or_else(|| Error::invalid("`ids` must be a list"))?;
                    storage::update(
                        "notes",
                        id,
                        &json!({"note": args.str("text")?})
                            .as_object()
                            .unwrap()
                            .clone(),
                    )?;
                }
                Ok(json!({"marked": ids.len()}))
            }
            "record" | "record_fail" | "chain" => {
                storage::insert(
                    "notes",
                    &json!({"note": serde_json::to_string(&args.0)?})
                        .as_object()
                        .unwrap()
                        .clone(),
                )?;
                if operation == "record_fail" {
                    return Err(Error::invalid("private-plugin-error"));
                }
                if operation == "chain" {
                    terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                }
                Ok(Value::Null)
            }
            "emit_write" => {
                let id = storage::insert(
                    "notes",
                    &json!({"note":"committed"}).as_object().unwrap().clone(),
                )?;
                terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                Ok(json!({"id":id}))
            }
            "emit_ok" => {
                terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                Ok(Value::Null)
            }
            "emit_then_fail" | "emit_then_trap" => {
                terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                if operation == "emit_then_trap" {
                    panic!("trap after emit");
                }
                Err(Error::invalid("failed after emit"))
            }
            "emit_undeclared" => {
                terrarium_guest::events::emit("other", &json!({}))?;
                Ok(Value::Null)
            }
            "emit_bad_payload" => {
                terrarium_guest::events::emit("pinged", &json!({"n":"wrong"}))?;
                Ok(Value::Null)
            }
            "emit_payload" => {
                terrarium_guest::events::emit(
                    "pinged",
                    args.get("payload").unwrap_or(&Value::Null),
                )?;
                Ok(Value::Null)
            }
            "emit_many" | "emit_count" => {
                let n = args.get("count").and_then(Value::as_u64).unwrap_or(51);
                for _ in 0..n {
                    terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                }
                Ok(Value::Null)
            }
            "emit_failed_child_budget" => {
                let failed = terrarium_guest::interop::call(
                    "event_sink",
                    "emit_count",
                    &json!({"count":51}),
                );
                if failed.is_ok() {
                    return Err(Error::invalid("expected child failure"));
                }
                terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                Ok(Value::Null)
            }
            "emit_child"
            | "emit_child_then_fail"
            | "emit_child_budget"
            | "emit_child_then_trap" => {
                terrarium_guest::interop::call("event_sink", "emit_count", &json!({"count":50}))?;
                if operation == "emit_child_then_trap" {
                    panic!("outer traps");
                }
                if operation == "emit_child_then_fail" {
                    return Err(Error::invalid("outer failed"));
                }
                if operation == "emit_child_budget" {
                    terrarium_guest::events::emit("pinged", &json!({"n":1}))?;
                }
                Ok(Value::Null)
            }
            "http_available" => Ok(json!(terrarium_guest::http::available())),
            "on_fetched" => {
                storage::insert(
                    "notes",
                    json!({"note":json!(&args.0).to_string()})
                        .as_object()
                        .unwrap(),
                )?;
                if args
                    .get("context")
                    .is_some_and(|c| c["enqueue"].as_bool() == Some(true))
                {
                    terrarium_guest::http::fetch(
                        &json!({"method":"GET","url":"https://example.com/","context":null}),
                        "on_fetched",
                    )?;
                }
                Ok(Value::Null)
            }
            "nested_fetch_then_fail" => {
                terrarium_guest::interop::call("net-fixture", "fetch_ok", &json!({}))?;
                Err(Error::invalid("outer failed"))
            }
            "fetch_raw" => terrarium_guest::http::fetch(
                args.get("request").unwrap_or(&Value::Null),
                "on_fetched",
            )
            .map(|id| json!(id)),
            "fetch_ok"
            | "fetch_commit_failure"
            | "fetch_then_fail"
            | "fetch_then_trap"
            | "fetch_other_host"
            | "fetch_bad_callback" => {
                if operation == "fetch_commit_failure" {
                    // The test installs a synthetic RAISE(ROLLBACK) trigger.
                    assert!(storage::insert(
                        "notes",
                        json!({"note":"synthetic"}).as_object().unwrap()
                    )
                    .is_err());
                }
                let url = if operation == "fetch_other_host" {
                    "https://other.org/"
                } else {
                    "https://example.com/"
                };
                let callback = if operation == "fetch_bad_callback" {
                    "missing"
                } else {
                    "on_fetched"
                };
                let id =
                    terrarium_guest::http::fetch(&json!({"method":"GET", "url":url}), callback)?;
                if operation == "fetch_then_fail" {
                    return Err(Error::invalid("failure after fetch"));
                }
                if operation == "fetch_then_trap" {
                    panic!("trap after fetch");
                }
                Ok(json!(id))
            }
            "ok" => Ok(json!({ "ok": true })),
            "oversized_reply" => {
                storage::insert(
                    "notes",
                    &json!({"note":"must roll back"})
                        .as_object()
                        .unwrap()
                        .clone(),
                )?;
                Ok(json!({"data": "x".repeat(4 * 1024 * 1024 + 1)}))
            }
            "oversized_writes" => {
                let payload = json!({"note":"x".repeat(1024 * 1024)})
                    .as_object()
                    .unwrap()
                    .clone();
                for _ in 0..34 {
                    storage::insert("notes", &payload)?;
                }
                Ok(Value::Null)
            }
            "spin" => {
                let mut n: u64 = 0;
                loop {
                    n = std::hint::black_box(n.wrapping_add(1));
                }
            }
            "grow" => {
                let mut chunks: Vec<Vec<u8>> = Vec::new();
                loop {
                    chunks.push(vec![1u8; 4 * 1024 * 1024]);
                    std::hint::black_box(&chunks);
                }
            }
            "insert_then_fail" => {
                let mut values = Map::new();
                values.insert("note".into(), json!("should be rolled back"));
                storage::insert("notes", &values)?;
                Err(Error::invalid("failing after a write"))
            }
            "insert_then_trap" => {
                let mut values = Map::new();
                values.insert("note".into(), json!("should be rolled back"));
                storage::insert("notes", &values)?;
                panic!("trap after a write")
            }
            "spam" => {
                for _ in 0..10_000 {
                    storage::query("SELECT 1", &[])?;
                }
                Ok(Value::Null)
            }
            "make_task" => terrarium_guest::interop::call(
                "tasks",
                "create",
                &json!({"title": "from another plugin"}),
            ),
            "undeclared" => terrarium_guest::interop::call("habits", "summary", &json!({})),
            "forbidden_op" => {
                terrarium_guest::interop::call("tasks", "delete", &json!({"id": "x"}))
            }
            "make_then_fail" => {
                terrarium_guest::interop::call(
                    "tasks",
                    "create",
                    &json!({"title": "kept by todo"}),
                )?;
                Err(Error::invalid("caller fails after the call"))
            }
            "write_sql" => Ok(json!(storage::query("DELETE FROM notes", &[]).is_err())),
            _ => Err(Error::unknown_operation(operation)),
        }
    }
}

terrarium_guest::plugin!(Limits);
