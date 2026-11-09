//! Test fixture: operations that misbehave on purpose.

use terrarium_guest::{storage, Args, Error, Plugin, Result};
use serde_json::{json, Map, Value};

struct Limits;

impl Plugin for Limits {
    fn handle(operation: &str, _args: Args) -> Result<Value> {
        match operation {
            "ok" => Ok(json!({ "ok": true })),
            "oversized_reply" => {
                storage::insert("notes", &json!({"note":"must roll back"}).as_object().unwrap().clone())?;
                Ok(json!({"data": "x".repeat(4 * 1024 * 1024 + 1)}))
            }
            "oversized_writes" => {
                let payload = json!({"note":"x".repeat(1024 * 1024)}).as_object().unwrap().clone();
                for _ in 0..34 { storage::insert("notes", &payload)?; }
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
            "forbidden_op" => terrarium_guest::interop::call("tasks", "delete", &json!({"id": "x"})),
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
