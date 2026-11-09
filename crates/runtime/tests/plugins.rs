//! The first-party plugins working together in one host: Bookmarks,
//! Habits (with reminders), Notes (sending tasks to To-do), and Reports
//! (reading To-do, Habits and Notes through their interfaces).

mod support;

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use tempfile::TempDir;
use terrarium_runtime::ai::{AiConfig, AiError, Completion, Sealer};
use terrarium_runtime::{build_plugin, HostConfig, PluginHost, RuntimeError};

const PLUGINS: [&str; 6] = ["todo", "bookmarks", "habits", "notes", "reports", "voice"];

fn package(name: &str) -> PathBuf {
    support::package_root().join(name).canonicalize().unwrap()
}

fn built() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        for name in PLUGINS {
            build_plugin(&package(name)).unwrap_or_else(|e| panic!("building {name}: {e}"));
        }
    });
}

fn host(data: &TempDir) -> PluginHost {
    host_with_speech(data, None)
}

fn host_with_speech(data: &TempDir, speech: Option<terrarium_runtime::SpeechConfig>) -> PluginHost {
    built();
    let mut host = PluginHost::new(HostConfig {
        data_dir: data.path().join("plugins"),
        spaces_dir: None,
        space: "personal".into(),
        device: "test-device".into(),
        user: "owner".into(),
        timezone: "Asia/Kolkata".into(),
        execution: Default::default(),
        speech,
    })
    .unwrap();
    for name in PLUGINS {
        host.load(&package(name)).unwrap();
    }
    host
}

fn params(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn today() -> String {
    // The host clock's notion of today in the configured timezone.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let ts = jiff::Timestamp::from_millisecond(now).unwrap();
    ts.to_zoned(jiff::tz::TimeZone::get("Asia/Kolkata").unwrap())
        .date()
        .to_string()
}

fn add_days(day: &str, n: i64) -> String {
    let date = day.parse::<jiff::civil::Date>().unwrap();
    date.checked_add(jiff::Span::new().days(n))
        .unwrap()
        .to_string()
}

fn weekday(day: &str) -> i64 {
    day.parse::<jiff::civil::Date>()
        .unwrap()
        .weekday()
        .to_monday_zero_offset() as i64
}

#[test]
fn bookmarks_validate_count_visits_and_search() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id = host
        .call(
            "bookmarks",
            "create",
            &json!({"url": "docs.rs/jiff", "tags": "Rust, docs, rust"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("bookmarks", "visit", &json!({"id": id})).unwrap();
    host.call("bookmarks", "visit", &json!({"id": id})).unwrap();
    let rows = host
        .query("bookmarks", "list", &params(json!({"q": "rust"})))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["url"], json!("https://docs.rs/jiff"));
    assert_eq!(rows[0]["title"], json!("docs.rs"));
    assert_eq!(rows[0]["tags"], json!("rust, docs"));
    assert_eq!(rows[0]["visits"], json!(2));
    for good in [
        "example.com:8080/path",
        "http://localhost:3000",
        "https://sub.example.org/?q=1#x",
    ] {
        host.call("bookmarks", "create", &json!({"url": good}))
            .unwrap_or_else(|e| panic!("{good}: {e}"));
    }
    for bad in [
        "javascript:alert(1)",
        "data:text/html,hi",
        "ftp://x.com",
        "https://",
        "not a url",
        "https://-x.com",
        "https://x.com:99999",
    ] {
        let err = host
            .call("bookmarks", "create", &json!({"url": bad}))
            .expect_err(bad);
        assert!(
            matches!(err, RuntimeError::Plugin { ref code, .. } if code == "invalid"),
            "{bad}: {err:?}"
        );
    }
}

#[test]
fn bookmarks_quick_save_projects_tags_and_pins() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let save = |host: &mut PluginHost, args: Value| -> String {
        host.call("bookmarks", "create", &args).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let book = save(
        &mut host,
        json!({"text": "The Rust book – https://doc.rust-lang.org/book/?utm_source=x&ch=2 #Rust #reading", "project": "Learning"}),
    );
    let jiff = save(
        &mut host,
        json!({"text": "docs.rs/jiff #rust", "project": "Learning"}),
    );
    let news = save(
        &mut host,
        json!({"url": "news.ycombinator.com", "tags": "#news"}),
    );

    // Same address again: no copy.
    let again = host
        .call(
            "bookmarks",
            "create",
            &json!({"text": "https://doc.rust-lang.org/book/?ch=2"}),
        )
        .unwrap();
    assert_eq!(again["id"], json!(book));
    assert_eq!(again["message"], json!("Already saved"));
    let err = host
        .call(
            "bookmarks",
            "create",
            &json!({"text": "no address here #x"}),
        )
        .expect_err("needs an address");
    assert!(matches!(err, RuntimeError::Plugin { ref code, .. } if code == "invalid"));

    let list = |host: &PluginHost, project: &str, tag: &str, sort: &str| {
        let mut p = params(json!({"q": "", "project": project, "tag": tag, "sort": sort}));
        p.extend(voice_day());
        host.query("bookmarks", "list", &p).unwrap()
    };
    let rows = list(&host, "", "", "new");
    assert_eq!(rows.len(), 3);
    let first = rows.iter().find(|r| r["id"] == json!(book)).unwrap();
    assert_eq!(first["title"], json!("The Rust book"));
    assert_eq!(first["url"], json!("https://doc.rust-lang.org/book/?ch=2"));
    assert_eq!(first["host"], json!("doc.rust-lang.org"));
    assert_eq!(first["tags_label"], json!("#rust #reading"));
    assert_eq!(first["section"], Value::Null);

    // Filters: project, exact tag (not a prefix), both.
    assert_eq!(list(&host, "Learning", "", "new").len(), 2);
    assert_eq!(list(&host, "", "rust", "new").len(), 2);
    assert_eq!(list(&host, "", "rus", "new").len(), 0);
    assert_eq!(list(&host, "Learning", "reading", "new").len(), 1);
    let projects = host
        .query(
            "bookmarks",
            "projects",
            &params(json!({"project": "Learning"})),
        )
        .unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(
        (projects[0]["label"].clone(), projects[0]["count"].clone()),
        (json!("All"), json!(3))
    );
    assert_eq!(
        (
            projects[1]["label"].clone(),
            projects[1]["selected"].clone()
        ),
        (json!("Learning"), json!(1))
    );
    let tags = host
        .query(
            "bookmarks",
            "tags",
            &params(json!({"project": "", "tag": "news"})),
        )
        .unwrap();
    let tag = |name: &str| {
        tags.iter()
            .find(|t| t["tag"] == json!(name))
            .unwrap()
            .clone()
    };
    assert_eq!(tag("rust")["count"], json!(2));
    assert_eq!(tag("rust")["next"], json!("rust"));
    assert_eq!(
        tag("news")["next"],
        json!(""),
        "clicking the selected tag clears it"
    );

    // Pin flips; pinned rows come first under their own heading.
    let pinned = host.call("bookmarks", "pin", &json!({"id": news})).unwrap();
    assert_eq!(pinned["message"], json!("Pinned"));
    let rows = list(&host, "", "", "new");
    assert_eq!(rows[0]["id"], json!(news));
    assert_eq!(rows[0]["section"], json!("Pinned"));
    assert_eq!(rows[1]["section"], json!("Bookmarks"));
    assert_eq!(
        list(&host, "Learning", "", "new")[0]["section"],
        Value::Null
    );

    // Visits sort, and show in the sheet.
    host.call("bookmarks", "visit", &json!({"id": jiff}))
        .unwrap();
    host.call("bookmarks", "visit", &json!({"id": jiff}))
        .unwrap();
    let rows = list(&host, "", "", "visits");
    assert_eq!(
        (rows[0]["id"].clone(), rows[1]["id"].clone()),
        (json!(news), json!(jiff))
    );
    assert_eq!(rows[1]["visits_label"], json!("2 visits"));
    let mut p = params(json!({"id": jiff}));
    p.extend(voice_day());
    let sheet = host.query("bookmarks", "bookmark", &p).unwrap();
    let info = sheet[0]["info"].as_str().unwrap();
    assert!(
        info.starts_with("Added ") && info.contains(" · 2 visits · Last opened "),
        "{info}"
    );

    // Edit: clearing the title falls back to the host; clearing the project.
    host.call(
        "bookmarks",
        "update",
        &json!({"id": jiff, "title": "", "project": "", "tags": "rust, Time", "pinned": "true"}),
    )
    .unwrap();
    let sheet = host.query("bookmarks", "bookmark", &p).unwrap();
    assert_eq!(sheet[0]["title"], json!("docs.rs"));
    assert_eq!(sheet[0]["project"], json!(""));
    assert_eq!(sheet[0]["tags"], json!("rust, time"));
    assert_eq!(sheet[0]["pinned"], json!(1));
    let unpinned = host.call("bookmarks", "pin", &json!({"id": jiff})).unwrap();
    assert_eq!(unpinned["pinned"], json!(false));

    let summary = host.query("bookmarks", "summary", &Map::new()).unwrap();
    assert_eq!(summary[0]["label"], json!("3 links · 1 pinned · 1 project"));
    let recent = host.query("bookmarks", "recent", &Map::new()).unwrap();
    assert_eq!(recent[0]["pin_label"], json!("Pinned"));
    let found = host
        .query("bookmarks", "search", &params(json!({"q": "learning"})))
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["subtitle"], json!("doc.rust-lang.org · Learning"));
    assert_eq!(
        found[0]["url"],
        json!("https://doc.rust-lang.org/book/?ch=2")
    );
}

#[test]
fn habits_check_in_plan_reminders_and_summarize() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let day = today();
    let id = host
        .call("habits", "save", &json!({"title": "Stretch", "weekdays": "0,1,2,3,4,5,6", "reminder": "23:59", "notify_on": "both"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let rows = host
        .query(
            "habits",
            "today",
            &params(json!({"today": day, "weekday": weekday(&day)})),
        )
        .unwrap();
    assert_eq!(rows[0]["is_open"], json!(1));
    assert_eq!(rows[0]["scheduled"], json!(1));
    let summary = &host
        .query("habits", "day_summary", &params(json!({"today": day})))
        .unwrap()[0];
    assert_eq!(summary["label"], json!("0 of 1 done today"));

    // Saving planned a week of reminders; the host sees at least the next one.
    let due = host.reminders(0);
    assert!(!due.is_empty() && due.len() <= 7, "{} reminders", due.len());
    assert!(due.iter().all(|(space, plugin, r)| space == "personal"
        && plugin == "habits"
        && r["notify_on"] == json!("both")
        && r["title"] == json!("Stretch")));
    let before = due.len();

    host.call("habits", "checkin", &json!({"id": id, "status": "done"}))
        .unwrap();
    let rows = host
        .query(
            "habits",
            "today",
            &params(json!({"today": day, "weekday": weekday(&day)})),
        )
        .unwrap();
    assert_eq!(rows[0]["is_done"], json!(1));
    // Today's reminder (if still ahead) is withdrawn once the habit is done.
    assert!(host.reminders(0).len() <= before);

    // Newest check-in wins: undo, then the habit is open again.
    host.call("habits", "checkin", &json!({"id": id, "status": "cleared"}))
        .unwrap();
    let rows = host
        .query(
            "habits",
            "today",
            &params(json!({"today": day, "weekday": weekday(&day)})),
        )
        .unwrap();
    assert_eq!(rows[0]["is_open"], json!(1));
    host.call("habits", "checkin", &json!({"id": id, "status": "done"}))
        .unwrap();

    let heat = host
        .query("habits", "heatmap", &params(json!({"today": day})))
        .unwrap();
    assert_eq!(heat, vec![params(json!({"day": day, "count": 1}))]);

    let summary = host
        .call("habits", "summary", &json!({"start": day, "end": day}))
        .unwrap();
    assert_eq!(summary["rate"], json!(1.0));

    // Pausing removes future reminders on the next plan.
    host.call("habits", "save", &json!({"id": id, "paused": true}))
        .unwrap();
    assert!(host.reminders(0).is_empty());

    // Streaks walk back over scheduled days; a skipped day keeps the run.
    let yesterday = add_days(&day, -1);
    let before = add_days(&day, -2);
    host.call("habits", "save", &json!({"id": id, "paused": false}))
        .unwrap();
    host.call("habits", "checkin", &json!({"id": id, "day": before}))
        .unwrap();
    host.call(
        "habits",
        "checkin",
        &json!({"id": id, "day": yesterday, "status": "skipped"}),
    )
    .unwrap();
    let row = &host
        .query("habits", "today", &params(json!({"today": day})))
        .unwrap()[0];
    assert_eq!(row["streak"], json!(2));
    assert!(
        row["week"].as_str().unwrap().ends_with("dsd"),
        "{}",
        row["week"]
    );
    let detail = &host
        .query("habits", "habit", &params(json!({"id": id, "today": day})))
        .unwrap()[0];
    assert_eq!(detail["best"], json!(2));
    assert_eq!(detail["week"].as_str().unwrap().len(), 14);

    // Counted habits: progress below the target is partial, not done.
    let water = host
        .call(
            "habits",
            "save",
            &json!({"title": "Water", "target": "3", "unit": "glasses"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let step = host
        .call("habits", "add", &json!({"id": water, "by": 2}))
        .unwrap();
    assert_eq!(step["status"], json!("partial"));
    let water_row = |host: &mut PluginHost| {
        host.query("habits", "today", &params(json!({"today": day})))
            .unwrap()
            .into_iter()
            .find(|r| r["id"] == json!(water))
            .unwrap()
    };
    assert_eq!(water_row(&mut host)["progress_label"], json!("2/3 glasses"));
    assert_eq!(
        host.call("habits", "add", &json!({"id": water})).unwrap()["status"],
        json!("done")
    );
    assert_eq!(water_row(&mut host)["is_done"], json!(1));
    // A checkbox sends `done: false`; the habit is open again at zero.
    host.call("habits", "checkin", &json!({"id": water, "done": false}))
        .unwrap();
    assert_eq!(water_row(&mut host)["count"], json!(0));
    let summary = &host
        .query("habits", "day_summary", &params(json!({"today": day})))
        .unwrap()[0];
    assert_eq!(summary["label"], json!("1 of 2 done today"));

    host.call("habits", "sleep_save", &json!({"hours": 7.5}))
        .unwrap();
    let sleep = host.query("habits", "sleep_recent", &Map::new()).unwrap();
    assert_eq!(sleep[0]["hours"], json!(7.5));

    let err = host
        .call("habits", "checkin", &json!({"id": id, "day": "2999-01-01"}))
        .unwrap_err();
    assert!(matches!(err, RuntimeError::Plugin { .. }));
}

#[test]
fn notes_merge_text_and_send_tasks_to_todo() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let id = host
        .call(
            "notes",
            "create",
            &json!({"body": "# Trip plan\n- [ ] book train", "project": "travel"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let all = params(json!({"q": "", "project": "", "utc_offset_ms": 0, "today": "2026-11-02"}));
    let listed = &host.query("notes", "list", &all).unwrap()[0];
    assert_eq!(listed["title"], json!("Trip plan"));
    assert_eq!(listed["checklist"], json!("0/1"));
    // The editor shows a blank title while it just follows the first line.
    let note = &host
        .query("notes", "note", &params(json!({"id": id})))
        .unwrap()[0];
    assert_eq!(note["title"], json!(""));

    host.call(
        "notes",
        "update",
        &json!({"id": id, "body": "# Trip plan\n- [ ] book train\n- [ ] pack"}),
    )
    .unwrap();
    let note = &host
        .query("notes", "note", &params(json!({"id": id})))
        .unwrap()[0];
    assert!(note["body"].as_str().unwrap().ends_with("- [ ] pack"));

    host.call("notes", "to_task", &json!({"text": "- [ ] book train"}))
        .unwrap();
    let tasks = host
        .query(
            "todo",
            "tasks",
            &params(json!({"today": today(), "filter": "all", "q": ""})),
        )
        .unwrap();
    assert_eq!(tasks[0]["title"], json!("book train"));

    let listed = host
        .query(
            "notes",
            "list",
            &params(json!({"q": "", "project": "travel"})),
        )
        .unwrap();
    assert_eq!(listed.len(), 1);

    // A note opened as new and left empty is dropped; one with text stays.
    let empty = host.call("notes", "create", &json!({"body": ""})).unwrap();
    let removed = host
        .call("notes", "tidy", &json!({"id": empty["id"]}))
        .unwrap();
    assert_eq!(removed["removed"], json!(true));
    let kept = host.call("notes", "tidy", &json!({"id": id})).unwrap();
    assert_eq!(kept["removed"], json!(false));
}

#[test]
fn weekly_review_reads_other_plugins() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let day = today();
    let task = host
        .call("todo", "create", &json!({"title": "Ship phase 4"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("todo", "toggle", &json!({"id": task, "done": true}))
        .unwrap();
    let habit = host
        .call("habits", "save", &json!({"title": "Read"}))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("habits", "checkin", &json!({"id": habit}))
        .unwrap();
    host.call("notes", "create", &json!({"body": "Phase 4 notes"}))
        .unwrap();

    host.call("todo", "create", &json!({"title": "Tidy desk #home"}))
        .and_then(|t| host.call("todo", "toggle", &json!({"id": t["id"], "done": true})))
        .unwrap();

    let today_only = params(json!({"today": day}));
    let made = host
        .call("reports", "generate", &json!({"start": day, "end": day}))
        .unwrap();
    assert_eq!(made["message"], json!("Report ready"));
    let id = made["id"].as_str().unwrap().to_string();
    let reports = host.query("reports", "list", &today_only).unwrap();
    let report = &reports[0];
    assert_eq!(report["tasks_done"], json!(2));
    assert_eq!(report["tasks_note"], json!("▲ 2 vs the day before"));
    assert_eq!(report["tasks_tone"], json!("good"));
    assert_eq!(report["habits_value"], json!("100%"));
    assert_eq!(report["notes_count"], json!(1));
    let summary = report["summary"].as_str().unwrap();
    assert!(
        summary.contains("- Ship phase 4")
            && summary.contains("- Read: 1 of 1")
            && summary.contains("- Phase 4 notes"),
        "{summary}"
    );

    // Breakdowns for the report sheet.
    let by_id = params(json!({"id": id}));
    let daily = host.query("reports", "daily", &by_id).unwrap();
    assert_eq!(daily.len(), 1);
    assert_eq!(daily[0]["count"], json!(2));
    let tags = host.query("reports", "tags", &by_id).unwrap();
    assert_eq!(tags[0]["label"], json!("#home · 1"));
    let habits = host.query("reports", "habits", &by_id).unwrap();
    assert_eq!(
        (habits[0]["title"].clone(), habits[0]["label"].clone()),
        (json!("Read"), json!("1/1"))
    );
    assert_eq!(host.query("reports", "tasks", &by_id).unwrap().len(), 2);
    let highlights = host.query("reports", "highlights", &by_id).unwrap();
    assert!(
        highlights
            .iter()
            .any(|h| h["text"] == json!("Never missed Read")),
        "{highlights:?}"
    );

    // The reflection is kept when the same range is made again.
    host.call(
        "reports",
        "reflect",
        &json!({"id": id, "wins": "Shipped it", "focus": ""}),
    )
    .unwrap();
    let again = host
        .call("reports", "generate", &json!({"start": day, "end": day}))
        .unwrap();
    assert_eq!(
        (again["id"].clone(), again["message"].clone()),
        (json!(id), json!("Report updated"))
    );
    let sheet = host
        .query(
            "reports",
            "report",
            &params(json!({"id": id, "today": day})),
        )
        .unwrap();
    assert_eq!(sheet[0]["wins"], json!("Shipped it"));
    assert_eq!(sheet[0]["reflected_chip"], json!("Reflected"));
    assert!(sheet[0]["copy_text"]
        .as_str()
        .unwrap()
        .ends_with("## What went well\nShipped it"));

    // This week's report is titled as such, refreshes in place, and
    // compares with the same days last week.
    let week = host.call("reports", "generate", &json!({})).unwrap();
    let week_id = week["id"].as_str().unwrap();
    let refreshed = host
        .call("reports", "generate", &json!({"id": week_id}))
        .unwrap();
    assert_eq!(refreshed["id"], week["id"]);
    // (Asked by id: on days other than Monday, the one-day report above
    // is the latest.)
    let latest = host
        .query(
            "reports",
            "report",
            &params(json!({"id": week_id, "today": day})),
        )
        .unwrap();
    assert_eq!(latest[0]["title"], json!("This week"));
    assert!(latest[0]["tasks_note"]
        .as_str()
        .unwrap()
        .contains("last week"));
    for period in ["last_week", "7d", "month", "last_month"] {
        host.call("reports", "generate", &json!({"period": period}))
            .unwrap();
    }
    assert!(host
        .call("reports", "generate", &json!({"period": "decade"}))
        .is_err());
    assert!(host
        .call(
            "reports",
            "generate",
            &json!({"start": day, "end": "2000-01-01"})
        )
        .is_err());
    let trend = host.query("reports", "trend", &Map::new()).unwrap();
    assert!(!trend.is_empty() && trend.len() <= 2, "{trend:?}");

    host.call("reports", "delete", &json!({"id": id})).unwrap();
    assert!(host
        .query(
            "reports",
            "report",
            &params(json!({"id": id, "today": day}))
        )
        .unwrap()
        .is_empty());
}

#[test]
fn voice_text_captures_go_to_todo_and_notes_and_audio_waits_without_a_model() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let typed = host
        .call(
            "voice",
            "capture_text",
            &json!({"text": "Call the bank\nabout the card"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("voice", "send", &json!({"id": typed, "to": "todo"}))
        .unwrap();
    let tasks = host
        .query(
            "todo",
            "tasks",
            &params(json!({"today": today(), "filter": "all", "q": ""})),
        )
        .unwrap();
    assert_eq!(tasks[0]["title"], json!("Call the bank"));

    let note = host
        .call(
            "voice",
            "capture_text",
            &json!({"text": "Idea: weekly review on Sundays"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call(
        "voice",
        "send",
        &json!({"id": note, "to": "notes", "text": "Idea: weekly review on Sunday evenings"}),
    )
    .unwrap();
    let notes = host
        .query(
            "notes",
            "list",
            &params(json!({"q": "Sunday evenings", "project": ""})),
        )
        .unwrap();
    assert_eq!(notes.len(), 1);
    assert!(host
        .query("voice", "inbox", &voice_day())
        .unwrap()
        .is_empty());

    // No model on this host: the recording waits, and the job leaves it be.
    let wav = "UklGRiQAAABXQVZFZm10IBAAAAABAAEAgD4AAAB9AAACABAAZGF0YQAAAAA=";
    host.call(
        "voice",
        "capture_audio",
        &json!({"audio": wav, "mime": "audio/wav"}),
    )
    .unwrap();
    assert_eq!(
        host.call("voice", "transcribe_pending", &json!({}))
            .unwrap()["transcribed"],
        json!(0)
    );
    let inbox = host.query("voice", "inbox", &voice_day()).unwrap();
    assert_eq!(inbox[0]["is_queued"], json!(1));
    assert_eq!(inbox[0]["title"], json!("Recording"));
    assert_eq!(inbox[0]["section"], json!("Waiting for your computer"));
    let summary = host.query("voice", "summary", &Map::new()).unwrap();
    assert_eq!(summary[0]["label"], json!("1 waiting for your computer"));

    // A brain dump becomes one task per line, list markers dropped.
    let dump = host
        .call(
            "voice",
            "capture_text",
            &json!({"text": "- buy milk\n\n2. call Sam\n• [ ] book flights"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let capture = host
        .query(
            "voice",
            "capture",
            &params(json!({"id": dump, "utc_offset_ms": 0, "today": today()})),
        )
        .unwrap();
    assert_eq!(capture[0]["multi"], json!(1));
    assert_eq!(capture[0]["length"], json!("11 words"));
    let sent = host
        .call(
            "voice",
            "send",
            &json!({"id": dump, "to": "todo", "split": true}),
        )
        .unwrap();
    assert_eq!(sent["message"], json!("Sent 3 tasks to To-do"));
    let titles: Vec<Value> = host
        .query(
            "todo",
            "tasks",
            &params(json!({"today": today(), "filter": "all", "q": ""})),
        )
        .unwrap()
        .iter()
        .map(|t| t["title"].clone())
        .collect();
    for title in ["buy milk", "call Sam", "book flights"] {
        assert!(titles.contains(&json!(title)), "{title} in {titles:?}");
    }

    // Keep without sending, find it in history, bring it back.
    let kept = host
        .call(
            "voice",
            "capture_text",
            &json!({"text": "Nice sunset today"}),
        )
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    host.call("voice", "archive", &json!({"id": kept})).unwrap();
    let history = host
        .query(
            "voice",
            "history",
            &params(json!({"q": "sunset", "utc_offset_ms": 0, "today": today()})),
        )
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["dest"], json!("Kept"));
    let all = host
        .query(
            "voice",
            "history",
            &params(json!({"q": "", "utc_offset_ms": 0, "today": today()})),
        )
        .unwrap();
    assert_eq!(all.len(), 4);
    host.call("voice", "reopen", &json!({"id": kept})).unwrap();
    let inbox = host.query("voice", "inbox", &voice_day()).unwrap();
    assert_eq!(inbox[0]["title"], json!("Nice sunset today"));
    assert_eq!(inbox[0]["section"], json!("To review"));
    let summary = host.query("voice", "summary", &Map::new()).unwrap();
    assert_eq!(
        summary[0]["label"],
        json!("1 to review · 1 waiting for your computer")
    );

    // Retry needs the audio; typed text has none.
    assert!(host.call("voice", "retry", &json!({"id": kept})).is_err());
}

fn voice_day() -> Map<String, Value> {
    params(json!({"utc_offset_ms": 0, "today": today()}))
}

/// Real local transcription. Runs only with `TERRA_TEST_SPEECH` pointing at
/// a speech.json (`python`, `model_dir`, `ffmpeg`) on a Mac with Parakeet.
#[test]
fn voice_transcribes_with_local_parakeet() {
    let Some(config) = std::env::var_os("TERRA_TEST_SPEECH") else {
        eprintln!("skipped: set TERRA_TEST_SPEECH to a speech.json to run");
        return;
    };
    let config: terrarium_runtime::SpeechConfig =
        serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
    let data = TempDir::new().unwrap();
    let wav = data.path().join("speech.wav");
    let status = std::process::Command::new("say")
        .args([
            "-o",
            wav.to_str().unwrap(),
            "--data-format=LEI16@16000",
            "Remember to buy milk and eggs tomorrow.",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    use base64::Engine;
    let audio = base64::engine::general_purpose::STANDARD.encode(std::fs::read(&wav).unwrap());

    let mut host = host_with_speech(&data, Some(config));
    let started = std::time::Instant::now();
    host.call(
        "voice",
        "capture_audio",
        &json!({"audio": audio, "mime": "audio/wav"}),
    )
    .unwrap();
    let inbox = host.query("voice", "inbox", &Map::new()).unwrap();
    let text = inbox[0]["text"].as_str().unwrap_or_default().to_lowercase();
    eprintln!(
        "transcribed in {:.1}s: {text}",
        started.elapsed().as_secs_f32()
    );
    assert_eq!(inbox[0]["is_ready"], json!(1), "{inbox:?}");
    assert!(text.contains("milk") && text.contains("eggs"), "{text}");
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn with_ai(host: &mut PluginHost) {
    host.set_ai_sealer(Sealer::new("device", &[9; 32]));
    host.ai_mut()
        .set_config(AiConfig {
            model: "test-model".into(),
            ..AiConfig::default()
        })
        .unwrap();
    host.ai_mut().set_key("sk-test-0001").unwrap();
}

fn reply(text: &str) -> Result<Completion, AiError> {
    Ok(Completion {
        text: text.into(),
        model: "test-model".into(),
        input_tokens: 100,
        output_tokens: 20,
    })
}

#[test]
fn finished_tasks_keep_notes_in_the_tasks_project() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let task = host
        .call(
            "todo",
            "create",
            &json!({"title": "Pay rent #home every month"}),
        )
        .unwrap();
    let done = host
        .call("todo", "toggle", &json!({"id": task["id"], "done": true}))
        .unwrap();
    assert_eq!(done["ask_note"], task["id"], "a note is offered by default");
    let saved = host
        .call(
            "todo",
            "save_note",
            &json!({"id": task["id"], "text": "Paid via the bank app; ref 42."}),
        )
        .unwrap();
    let note_id = saved["note"].as_str().unwrap().to_string();

    // The next one in the series adds to the same note, newest first.
    let next = done["next"]["id"].clone();
    let task_row = host
        .query("todo", "task", &params(json!({"id": next})))
        .unwrap();
    assert_eq!(
        task_row[0]["last_note"],
        json!("Paid via the bank app; ref 42.")
    );
    host.call("todo", "toggle", &json!({"id": next, "done": true}))
        .unwrap();
    let again = host
        .call(
            "todo",
            "save_note",
            &json!({"id": next, "text": "Paid again."}),
        )
        .unwrap();
    assert_eq!(again["note"].as_str(), Some(note_id.as_str()));
    let note = host
        .query("notes", "note", &params(json!({"id": note_id})))
        .unwrap();
    let body = note[0]["body"].as_str().unwrap();
    assert!(body.starts_with("_From To-do · #home_"), "{body}");
    assert!(
        body.find("Paid again.").unwrap() < body.find("ref 42").unwrap(),
        "{body}"
    );
    assert_eq!(note[0]["project"], json!("Tasks"));

    // Kept apart: not in the main list or the reports' note counts.
    let day = today();
    let main = host
        .query(
            "notes",
            "list",
            &params(json!({"q": "", "project": "", "utc_offset_ms": 0, "today": day})),
        )
        .unwrap();
    assert!(main.is_empty(), "{main:?}");
    let tasks = host
        .query(
            "notes",
            "list",
            &params(json!({"q": "", "project": "Tasks", "utc_offset_ms": 0, "today": day})),
        )
        .unwrap();
    assert_eq!(tasks.len(), 1);

    // Asking only for tagged tasks.
    host.call(
        "todo",
        "note_settings",
        &json!({"ask": "tagged", "tags": "work"}),
    )
    .unwrap();
    let other = host
        .call("todo", "create", &json!({"title": "Water plants"}))
        .unwrap();
    let done = host
        .call("todo", "toggle", &json!({"id": other["id"], "done": true}))
        .unwrap();
    assert!(done["ask_note"].is_null());
    let work = host
        .call("todo", "create", &json!({"title": "Deploy #work"}))
        .unwrap();
    let done = host
        .call("todo", "toggle", &json!({"id": work["id"], "done": true}))
        .unwrap();
    assert_eq!(done["ask_note"], work["id"]);
}

#[test]
fn ai_reports_use_skills_chosen_data_and_schedules() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for (title, note) in [
        (
            "Ship billing fix #work",
            Some("Root cause was token expiry."),
        ),
        ("Dentist #private", Some("Molar.")),
        ("Groceries #home", None),
    ] {
        let t = host
            .call("todo", "create", &json!({"title": title}))
            .unwrap();
        host.call("todo", "toggle", &json!({"id": t["id"], "done": true}))
            .unwrap();
        if let Some(note) = note {
            host.call("todo", "save_note", &json!({"id": t["id"], "text": note}))
                .unwrap();
        }
    }
    host.call("todo", "create", &json!({"title": "Prepare review #work"}))
        .unwrap();

    // Without AI set up the report keeps its numbers and says why.
    let made = host
        .call(
            "reports",
            "generate",
            &json!({"skill": "builtin:manager", "period": "7d"}),
        )
        .unwrap();
    assert!(
        made["message"]
            .as_str()
            .unwrap()
            .contains("AI could not write it"),
        "{made}"
    );
    let id = made["id"].as_str().unwrap().to_string();
    let day = today();
    let report = |host: &PluginHost, id: &str| {
        host.query(
            "reports",
            "report",
            &params(json!({"id": id, "today": day})),
        )
        .unwrap()
        .remove(0)
    };
    let r = report(&host, &id);
    assert_eq!(r["draft_status"], json!("failed"));
    assert_eq!(r["tasks_done"], json!(3));
    assert!(r["title"]
        .as_str()
        .unwrap()
        .ends_with("· Update for my manager"));

    // With AI: only the skill's sources, no #private, tag filter applied.
    with_ai(&mut host);
    let sent = host.call("reports", "send", &json!({"id": id})).unwrap();
    assert_eq!(sent["message"], json!("Writing with AI…"));
    let (job, _, key) = host.take_ai_job().unwrap();
    assert_eq!(key.as_deref(), Some("sk-test-0001"));
    let facts = &job.prompt.prompt;
    assert!(
        facts.contains("Ship billing fix") && facts.contains("> Root cause was token expiry."),
        "{facts}"
    );
    assert!(
        facts.contains("## Open tasks") && facts.contains("Prepare review"),
        "{facts}"
    );
    assert!(
        !facts.contains("Dentist") && !facts.contains("Molar"),
        "{facts}"
    );
    assert!(
        !facts.contains("## Habits"),
        "manager skill leaves habits out"
    );
    assert!(
        job.prompt.system.contains("Never invent")
            && job.prompt.system.contains("status update for my manager")
    );
    assert!(host
        .deliver_ai(job, reply("## Done\n- Billing fix"))
        .is_none());
    let r = report(&host, &id);
    assert_eq!(r["draft_status"], json!("done"));
    assert_eq!(r["copy_text"], json!("## Done\n- Billing fix"));
    assert_eq!(r["draft_note"], json!("Written by test-model"));

    // Asking for changes sends the current text and what to change.
    host.call(
        "reports",
        "send",
        &json!({"id": id, "feedback": "Lead with blockers"}),
    )
    .unwrap();
    let (job, _, _) = host.take_ai_job().unwrap();
    let prompt = &job.prompt.prompt;
    assert!(
        prompt.contains("# Your previous draft\n\n## Done\n- Billing fix")
            && prompt.contains("Lead with blockers")
            && prompt.contains("Ship billing fix"),
        "{prompt}"
    );
    host.deliver_ai(job, reply("## Blockers\nNone"));
    assert_eq!(report(&host, &id)["draft"], json!("## Blockers\nNone"));

    // Same skill and range: the same report. Another skill: its own.
    let review = host
        .call(
            "reports",
            "generate",
            &json!({"skill": "builtin:self", "period": "7d", "tags": "work", "review": true}),
        )
        .unwrap();
    assert_ne!(review["id"], json!(id));
    let r = report(&host, review["id"].as_str().unwrap());
    assert_eq!(r["draft_status"], json!("ready"), "waits for a check");
    let facts = r["facts"].as_str().unwrap();
    assert!(
        facts.contains("Only work tagged #work") && !facts.contains("Groceries"),
        "{facts}"
    );
    assert!(host.take_ai_job().is_none());

    // A schedule runs through the host; the run ends when the draft lands.
    let saved = host
        .call(
            "reports",
            "save_schedule",
            &json!({"name": "Friday update", "skill": "builtin:manager", "period": "7d", "every": "week", "days": "fri", "at": "17:00"}),
        )
        .unwrap();
    let schedule = saved["id"].as_str().unwrap().to_string();
    let listed = host.scheduler().list(None, Some("reports"));
    assert!(
        listed
            .iter()
            .any(|s| s.key == schedule && s.describe == "Every Fri at 17:00"),
        "{listed:?}"
    );
    host.call("reports", "run_schedule", &json!({"id": schedule}))
        .unwrap();
    assert!(host.run_due(now_ms(), true).is_empty(), "waits for the AI");
    let (job, _, _) = host.take_ai_job().unwrap();
    let run = job.run.clone().unwrap();
    assert_eq!(host.scheduler().run(&run).unwrap().status, "waiting");
    let ended = host.deliver_ai(job, reply("Scheduled text")).unwrap();
    assert_eq!(ended.run.status, "done");
    assert!(
        ended
            .notice
            .as_deref()
            .is_some_and(|n| n.ends_with("· Update for my manager is ready.")),
        "{ended:?}"
    );
    let rows = host.query("reports", "schedules", &Map::new()).unwrap();
    assert_eq!(rows[0]["status_chip"], json!("Last run done"));
    let made = report(&host, rows[0]["last_report"].as_str().unwrap());
    assert_eq!(made["draft"], json!("Scheduled text"));
    assert_eq!(
        made["id"],
        json!(id),
        "the same range and skill: one report"
    );

    // A temporary failure is retried; no key fails the run but keeps the report.
    host.call("reports", "run_schedule", &json!({"id": schedule}))
        .unwrap();
    host.run_due(now_ms(), true);
    let (job, _, _) = host.take_ai_job().unwrap();
    let run = job.run.clone().unwrap();
    assert!(host
        .deliver_ai(job, Err(AiError::new("rate-limited", "slow down")))
        .is_none());
    assert_eq!(host.scheduler().run(&run).unwrap().status, "retry");
    host.ai_mut().remove_key().unwrap();
    host.call("reports", "run_schedule", &json!({"id": schedule}))
        .unwrap();
    let ended = host.run_due(now_ms(), true);
    let failed = ended
        .iter()
        .find(|e| e.run.manual && e.run.status == "failed")
        .expect("failed run");
    assert!(
        failed.notify && failed.run.error.as_deref().unwrap().contains("API key"),
        "{failed:?}"
    );

    // Removing the schedule removes it from the host too.
    host.call("reports", "delete_schedule", &json!({"id": schedule}))
        .unwrap();
    assert!(host
        .scheduler()
        .list(None, Some("reports"))
        .iter()
        .all(|s| s.key != schedule));
}

#[test]
fn large_queued_audio_can_be_retried_without_relaxing_other_plugins_queries() {
    use base64::Engine;
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    let mut wav = b"RIFF\0\0\0\0WAVE".to_vec();
    wav.resize(8 * 1024 * 1024, 0);
    let audio = base64::engine::general_purpose::STANDARD.encode(wav);
    let created = host
        .call(
            "voice",
            "capture_audio",
            &json!({"audio":audio,"mime":"audio/wav"}),
        )
        .unwrap();
    let retried = host
        .call("voice", "retry", &json!({"id":created["id"]}))
        .unwrap();
    assert_eq!(retried["status"], "queued");
}

#[test]
fn all_plugins_reject_hostile_inputs_and_remain_usable() {
    let data = TempDir::new().unwrap();
    let mut host = host(&data);
    for (plugin, operation, input) in [
        (
            "todo",
            "create",
            json!({"title":"Task", "remind_time":"+1:00"}),
        ),
        ("todo", "create", json!({"title":"x".repeat(501)})),
        ("bookmarks", "create", json!({"url":"javascript:alert(1)"})),
        (
            "bookmarks",
            "create",
            json!({"url":"https://example.com/\nattack"}),
        ),
        ("notes", "create", json!({"body":"x".repeat(200_001)})),
        ("notes", "create", json!({"title":"x".repeat(201)})),
        (
            "habits",
            "save",
            json!({"title":"Habit", "reminder":"+1:00", "weekdays":[7]}),
        ),
        (
            "reports",
            "generate",
            json!({"period":"custom", "start":"2026-99-99", "end":"2026-11-07"}),
        ),
        ("voice", "capture_text", json!({"text":"   "})),
        (
            "voice",
            "capture_audio",
            json!({"audio":"not-base64", "mime":"audio/wav"}),
        ),
    ] {
        assert!(
            host.call(plugin, operation, &input).is_err(),
            "accepted {plugin}/{operation}: {input}"
        );
    }
    assert!(host
        .call(
            "todo",
            "create",
            &json!({"title":"Still usable", "remind_time":"09:30"})
        )
        .is_ok());
    assert!(host
        .call("notes", "create", &json!({"body":"Valid note"}))
        .is_ok());
}
