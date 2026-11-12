#![cfg(feature = "plugin-integration")]
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tempfile::TempDir;
use terrarium_core::StoreSet;
use terrarium_runtime::{build_plugin, services, Execution, HostConfig, PluginHost};

fn fixture() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/limits");
            build_plugin(&p).unwrap();
            p
        })
        .clone()
}
fn host(dir: &TempDir, device: &str) -> PluginHost {
    let mut h = PluginHost::new(HostConfig {
        data_dir: dir.path().to_owned(),
        spaces_dir: None,
        space: "personal".into(),
        device: device.into(),
        user: "owner".into(),
        timezone: "Asia/Kolkata".into(),
        execution: Execution::Auto,
        speech: None,
    })
    .unwrap();
    h.load(&fixture()).unwrap();
    h.set_automation_context(true);
    h
}
fn service(h: &mut PluginHost, cmd: &str, args: Value) -> Value {
    services::invoke(h, cmd, &args, true).unwrap().unwrap()
}
fn rule(h: &mut PluginHost, op: &str) -> String {
    let r = service(
        h,
        "rule_create",
        json!({"space":"personal","rule":{"name":"Synthetic rule","trigger":{"plugin":"limits","event":"pinged"},"action":{"plugin":"limits","operation":op,"args":{"n":"{{event.n}}"}}}}),
    );
    r["id"].as_str().unwrap().to_owned()
}
fn approve(h: &mut PluginHost, id: &str) {
    let r = service(h, "rule_review", json!({"space":"personal","id":id}));
    service(
        h,
        "rule_approve",
        json!({"space":"personal","id":id,"digest":r["digest"]}),
    );
}
fn arm(h: &mut PluginHost, device: &str) {
    service(
        h,
        "rules_save_settings",
        json!({"space":"personal","primary_device":device,"paused":false}),
    );
}
fn emit(h: &mut PluginHost) {
    h.call_in("personal", "limits", "emit_ok", &json!({}))
        .unwrap();
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}
fn notes(h: &PluginHost) -> Vec<Map<String, Value>> {
    h.query_in("personal", "limits", "all", &Map::new())
        .unwrap()
}

#[test]
fn matching_rule_runs_action_once_per_event() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    assert_eq!(h.run_rules(now())[0].status, "done");
    assert!(h.run_rules(now()).is_empty());
    assert_eq!(notes(&h).len(), 1);
    assert_eq!(notes(&h)[0]["note"], r#"{"n":1}"#);
    let audit = std::fs::read_to_string(d.path().join("automation-audit.jsonl")).unwrap();
    let line: Value = serde_json::from_str(audit.trim()).unwrap();
    assert_eq!(line.as_object().unwrap().len(), 9);
    assert_eq!(line["change_count"], 2);
    assert!(
        !audit.contains("Synthetic rule") && !audit.contains("payload") && !audit.contains("args")
    );
}
#[test]
fn non_primary_device_does_not_run_rules() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "b");
    emit(&mut h);
    assert!(h.run_rules(now()).is_empty());
    assert!(notes(&h).is_empty());
    assert_eq!(
        service(&mut h, "rules", json!({"space":"personal"}))["rules"][0]["waiting"],
        1
    );
}
#[test]
fn unapproved_or_disabled_rule_does_not_run() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    arm(&mut h, "a");
    emit(&mut h);
    assert!(h.run_rules(now()).is_empty());
    approve(&mut h, &id);
    service(
        &mut h,
        "rule_set_enabled",
        json!({"space":"personal","id":id,"enabled":false}),
    );
    emit(&mut h);
    assert!(h.run_rules(now()).is_empty());
    assert!(notes(&h).is_empty());
}
#[test]
fn chain_stops_at_depth_limit() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "chain");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    for _ in 0..3 {
        assert_eq!(h.run_rules(now())[0].status, "done");
    }
    let stopped = h.run_rules(now());
    assert_eq!(stopped[0].status, "stopped");
    assert_eq!(stopped[0].code.as_deref(), Some("stopped"));
    assert_eq!(notes(&h).len(), 3);
    assert!(h.pending_events(10).unwrap().is_empty());
}
#[test]
fn failed_action_records_failed_run_and_rolls_back() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record_fail");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    let runs = h.run_rules(now());
    assert_eq!(runs[0].status, "failed");
    assert!(runs[0].notify);
    assert!(notes(&h).is_empty());
    emit(&mut h);
    assert!(!h.run_rules(now())[0].notify);
    assert!(
        !std::fs::read_to_string(d.path().join("automation-audit.jsonl"))
            .unwrap()
            .contains("private-plugin-error")
    );
}
#[test]
fn rate_limit_skips_after_sixty_runs_in_an_hour() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    for _ in 0..61 {
        emit(&mut h);
    }
    let runs = h.run_rules(now());
    assert_eq!(runs.len(), 61);
    assert_eq!(notes(&h).len(), 60);
    assert_eq!(runs[60].code.as_deref(), Some("rate"));
    // Removing synced history cannot reset local accounting. Clock rollback cannot either.
    h.rules_mut()
        .store_mut()
        .write(|w| w.delete("runs", &format!("{}@{}", runs[59].rule, runs[59].event)))
        .unwrap();
    emit(&mut h);
    assert_eq!(
        h.run_rules(now().saturating_sub(1000))[0].code.as_deref(),
        Some("rate")
    );
}
#[test]
fn same_run_id_on_two_devices_converges() {
    let da = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let mut a = host(&da, "a");
    let mut b = host(&db, "b");
    let id = rule(&mut a, "record");
    approve(&mut a, &id);
    arm(&mut a, "a");
    emit(&mut a);
    let (events, _) = a.store_mut("_events").unwrap().export_since(0).unwrap();
    let (rules, _) = a.rules().store().export_since(0).unwrap();
    b.store_mut("_events").unwrap().import(&events).unwrap();
    b.rules_mut().store_mut().import(&rules).unwrap();
    approve(&mut b, &id);
    arm(&mut b, "b");
    // Events predate B's new-events cutoff: explicitly use a fresh emission shared offline.
    emit(&mut a);
    let (events, _) = a.store_mut("_events").unwrap().export_since(0).unwrap();
    b.store_mut("_events").unwrap().import(&events).unwrap();
    a.run_rules(now());
    b.run_rules(now());
    // Offline primaries may both act: convergence is history deduplication only.
    assert_eq!(notes(&a).len(), 2);
    assert_eq!(notes(&b).len(), 1);
    let (aa, _) = a.rules().store().export_since(0).unwrap();
    let (bb, _) = b.rules().store().export_since(0).unwrap();
    a.rules_mut().store_mut().import(&bb).unwrap();
    b.rules_mut().store_mut().import(&aa).unwrap();
    let ra = a.rules().runs("personal", Some(&id), 50, None).unwrap();
    let rb = b.rules().runs("personal", Some(&id), 50, None).unwrap();
    assert_eq!(ra, rb);
    let ids: std::collections::BTreeSet<_> = ra["runs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].clone().to_string())
        .collect();
    assert_eq!(ids.len(), ra["runs"].as_array().unwrap().len());
}
#[test]
fn replay_after_restart_and_history_deletion_does_not_invoke_again() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    let (events, _) = h.store_mut("_events").unwrap().export_since(0).unwrap();
    let run = h.run_rules(now()).remove(0);
    h.rules_mut()
        .store_mut()
        .write(|w| w.delete("runs", &format!("{}@{}", id, run.event)))
        .unwrap();
    drop(h);
    let mut h = host(&d, "a");
    h.store_mut("_events").unwrap().import(&events).unwrap();
    // Simulate peer replay of unhandled event while preserving its id.
    h.store_mut("_events")
        .unwrap()
        .write(|w| {
            w.update(
                "events",
                &run.event,
                json!({"handled_ms":null}).as_object().unwrap(),
            )
        })
        .unwrap();
    assert!(h.run_rules(now()).is_empty());
    assert_eq!(notes(&h).len(), 1);
}
#[test]
fn keyless_context_and_synced_reassignment_require_local_authority() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    h.set_automation_context(false);
    assert!(h.run_rules(now()).is_empty());
    h.set_automation_context(true);
    h.rules_mut()
        .set_primary_device("personal", Some("b"))
        .unwrap();
    h.run_rules(now());
    h.rules_mut()
        .set_primary_device("personal", Some("a"))
        .unwrap();
    assert!(h.run_rules(now()).is_empty());
    arm(&mut h, "a");
    assert_eq!(h.run_rules(now()).len(), 1);
}

#[test]
fn interrupted_claim_is_published_without_invoking() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    // Simulate a crash after durable claiming and before the action starts.
    let event = h.pending_events(1).unwrap().remove(0);
    let at = now();
    let summary = json!({"at_ms":at,"space":"personal","rule":id,"event":event.id,
        "plugin":"limits","operation":"record","status":"failed","code":"interrupted","change_count":0});
    let history = json!({"space":"personal","rule":id,"rule_name":"Synthetic rule",
        "event":event.id,"status":"failed","error":"interrupted","device":"a",
        "started_ms":at,"finished_ms":at});
    let conn = rusqlite::Connection::open(d.path().join("automation-local.sqlite")).unwrap();
    conn.execute("INSERT INTO claims (id,rule,digest,at,attempted,outcome,history) VALUES (?1,?2,'synthetic',?3,1,?4,?5)",
        rusqlite::params![format!("{}@{}", id, event.id),id,at as i64,summary.to_string(),history.to_string()]).unwrap();
    drop(conn);
    drop(h);
    let mut h = host(&d, "a");
    let runs = h.run_rules(now());
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].code.as_deref(), Some("interrupted"));
    assert!(notes(&h).is_empty());
    assert!(h.run_rules(now()).is_empty());
    let info = service(&mut h, "rules", json!({"space":"personal"}));
    assert_eq!(info["rules"][0]["last_run"]["error"], "interrupted");
}

#[test]
fn unwritable_audit_prevents_claim_and_action() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    let audit = d.path().join("automation-audit.jsonl");
    std::fs::create_dir(&audit).unwrap();
    assert!(h.run_rules(now()).is_empty());
    assert!(h.automation_problem().is_some());
    assert!(notes(&h).is_empty());
    assert_eq!(h.pending_events(1).unwrap().len(), 1);
    std::fs::remove_dir(audit).unwrap();
    assert_eq!(h.run_rules(now())[0].status, "done");
    assert!(h.automation_problem().is_none());
}

#[test]
fn fanout_stops_further_matches_in_rule_id_order() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let mut ids = Vec::new();
    for _ in 0..11 {
        let id = rule(&mut h, "record");
        approve(&mut h, &id);
        ids.push(id);
    }
    ids.sort();
    arm(&mut h, "a");
    emit(&mut h);
    let runs = h.run_rules(now());
    assert_eq!(
        runs.iter().map(|r| &r.rule).collect::<Vec<_>>(),
        ids.iter().collect::<Vec<_>>()
    );
    assert!(runs[..10].iter().all(|r| r.status == "done"));
    assert_eq!(runs[10].status, "stopped");
    assert_eq!(runs[10].code.as_deref(), Some("fanout"));
    assert_eq!(notes(&h).len(), 10);
}

#[test]
fn tick_processes_only_two_hundred_events() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    for count in [50, 50, 50, 50, 1] {
        h.call_in("personal", "limits", "emit_count", &json!({"count":count}))
            .unwrap();
    }
    assert_eq!(h.run_rules(now()).len(), 200);
    assert_eq!(h.pending_events(10).unwrap().len(), 1);
    assert_eq!(h.run_rules(now()).len(), 1);
    assert!(h.pending_events(10).unwrap().is_empty());
    assert_eq!(notes(&h).len(), 60);
}

#[test]
fn approval_cutoff_and_synced_enable_cannot_replay_old_events() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    emit(&mut h);
    let event = h.pending_events(1).unwrap().remove(0);
    h.store_mut("_events")
        .unwrap()
        .write(|w| {
            w.update(
                "events",
                &event.id,
                json!({"emitted_ms":now() - 1000}).as_object().unwrap(),
            )
        })
        .unwrap();
    approve(&mut h, &id);
    arm(&mut h, "a");
    assert!(h.run_rules(now()).is_empty());
    assert!(h.pending_events(1).unwrap().is_empty());
    service(
        &mut h,
        "rule_set_enabled",
        json!({"space":"personal","id":id,"enabled":false}),
    );
    // Synced enable changes configuration but cannot restore the local execution grant.
    h.rules_mut().set_enabled(&id, true).unwrap();
    emit(&mut h);
    assert!(h.run_rules(now()).is_empty());
    assert!(notes(&h).is_empty());
    service(
        &mut h,
        "rule_set_enabled",
        json!({"space":"personal","id":id,"enabled":true}),
    );
    emit(&mut h);
    assert_eq!(h.run_rules(now())[0].status, "done");
}

#[test]
fn closed_space_stays_pending_until_reopened() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let space = h.spaces_mut().create("Synthetic space").unwrap().id;
    h.sync_spaces().unwrap();
    let r = service(
        &mut h,
        "rule_create",
        json!({"space":space,"rule":{
        "name":"Closed space rule","trigger":{"plugin":"limits","event":"pinged"},
        "action":{"plugin":"limits","operation":"record","args":{"n":"{{event.n}}"}}}}),
    );
    let id = r["id"].clone();
    let r = service(&mut h, "rule_review", json!({"space":space,"id":id}));
    service(
        &mut h,
        "rule_approve",
        json!({"space":space,"id":id,"digest":r["digest"]}),
    );
    service(
        &mut h,
        "rules_save_settings",
        json!({"space":space,"primary_device":"a","paused":false}),
    );
    h.call_in(&space, "limits", "emit_ok", &json!({})).unwrap();
    h.spaces_mut().set_removed(&space, true).unwrap();
    h.sync_spaces().unwrap();
    assert!(h.run_rules(now()).is_empty());
    assert_eq!(h.pending_events(10).unwrap().len(), 1);
    h.spaces_mut().set_removed(&space, false).unwrap();
    h.sync_spaces().unwrap();
    assert_eq!(h.run_rules(now())[0].status, "done");
    assert_eq!(
        h.query_in(&space, "limits", "all", &Map::new())
            .unwrap()
            .len(),
        1
    );
    assert!(notes(&h).is_empty());
}

#[test]
fn local_claim_write_failure_and_capacity_prevent_actions() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    emit(&mut h);
    let conn = rusqlite::Connection::open(d.path().join("automation-local.sqlite")).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_claim BEFORE INSERT ON claims BEGIN SELECT RAISE(FAIL,'synthetic failure'); END;").unwrap();
    assert!(h.run_rules(now()).is_empty());
    assert!(notes(&h).is_empty());
    assert_eq!(h.pending_events(1).unwrap().len(), 1);
    conn.execute_batch("DROP TRIGGER fail_claim;
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000)
        INSERT INTO claims (id,rule,digest,at,attempted,published) SELECT 'synthetic-'||x,'other','synthetic',0,0,1 FROM n;").unwrap();
    assert!(h.run_rules(now()).is_empty());
    assert_eq!(h.automation_problem(), Some("Automation history is full"));
    assert!(notes(&h).is_empty());
    assert_eq!(h.pending_events(1).unwrap().len(), 1);
}

#[test]
fn profile_rate_limit_counts_other_rules_and_survives_clock_rollback() {
    let d = TempDir::new().unwrap();
    let mut h = host(&d, "a");
    let id = rule(&mut h, "record");
    approve(&mut h, &id);
    arm(&mut h, "a");
    let at = now();
    let conn = rusqlite::Connection::open(d.path().join("automation-local.sqlite")).unwrap();
    conn.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<600)
        INSERT INTO claims (id,rule,digest,at,attempted,published) SELECT 'synthetic-'||x,'other','synthetic',?1,1,1 FROM n", [at as i64]).unwrap();
    emit(&mut h);
    assert_eq!(h.run_rules(at)[0].code.as_deref(), Some("rate"));
    emit(&mut h);
    assert_eq!(h.run_rules(at - 3_600_001)[0].code.as_deref(), Some("rate"));
    assert!(notes(&h).is_empty());
}
