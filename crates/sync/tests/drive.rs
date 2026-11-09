//! Google Drive store against a local fake of the Drive v3 endpoints it
//! uses, plus the OAuth sign-in flow and offline-host sync through Drive.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tempfile::TempDir;
use terrarium_core::{DeviceId, ManualTime, PluginSchema, Store, StoreConfig};
use terrarium_sync::{
    authorize_drive, BatchStore, DeviceKeys, DriveCredentials, DriveEndpoints, DriveStore,
    FolderStore, HttpStore, MultiStore, Roster, SpaceKey, SpaceSync,
};

#[derive(Default)]
struct Files {
    next: u64,
    files: Vec<(String, String, Vec<u8>)>, // id, name, bytes
    requests: u64,
}

fn decode(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

fn param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .map(decode)
}

/// Serves `/token`, `/api/files...` and `/upload/files...`.
fn fake_drive() -> (DriveEndpoints, Arc<Mutex<Files>>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let base = format!("http://{}", server.server_addr().to_ip().unwrap());
    let state = Arc::new(Mutex::new(Files::default()));
    let files = state.clone();
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let (path, query) = url.split_once('?').unwrap_or((&url, ""));
            let (path, query) = (path.to_string(), query.to_string());
            let mut body = Vec::new();
            request.as_reader().read_to_end(&mut body).unwrap();
            let authorized = request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer tok");
            let content_type = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Content-Type"))
                .map(|h| h.value.as_str().to_string())
                .unwrap_or_default();
            let mut db = files.lock().unwrap();
            db.requests += 1;
            let json = |v: Value| tiny_http::Response::from_string(v.to_string());
            let response = if path == "/token" {
                let form = String::from_utf8(body).unwrap();
                if form.contains("grant_type=authorization_code")
                    && !form.contains("code_verifier=")
                {
                    tiny_http::Response::from_string("missing verifier").with_status_code(400)
                } else {
                    json(
                        json!({"access_token": "tok", "expires_in": 3600, "refresh_token": "refresh-1"}),
                    )
                }
            } else if !authorized {
                tiny_http::Response::from_string("unauthorized").with_status_code(401)
            } else if path == "/api/files" && request.method() == &tiny_http::Method::Get {
                let q = param(&query, "q").unwrap();
                let (op, value) = if let Some(v) = q.strip_prefix("name = ") {
                    ("eq", v.trim_matches('\'').to_string())
                } else {
                    (
                        "contains",
                        q.strip_prefix("name contains ")
                            .unwrap()
                            .trim_matches('\'')
                            .to_string(),
                    )
                };
                let matches: Vec<Value> = db
                    .files
                    .iter()
                    .filter(|(_, name, _)| {
                        if op == "eq" {
                            *name == value
                        } else {
                            name.contains(&value)
                        }
                    })
                    .map(|(id, name, _)| json!({"id": id, "name": name}))
                    .collect();
                json(json!({ "files": matches }))
            } else if let Some(id) = path.strip_prefix("/api/files/") {
                match db.files.iter().find(|(fid, _, _)| fid == id) {
                    Some((_, _, bytes)) => tiny_http::Response::from_data(bytes.clone()),
                    None => tiny_http::Response::from_string("nope").with_status_code(404),
                }
            } else if path == "/upload/files" {
                let boundary = content_type.split("boundary=").nth(1).unwrap().to_string();
                let text = body.clone();
                let marker = format!("--{boundary}");
                let parts: Vec<&[u8]> = split(&text, marker.as_bytes());
                let meta_part = parts[1];
                let meta_start = find(meta_part, b"\r\n\r\n").unwrap() + 4;
                let meta: Value =
                    serde_json::from_slice(trim_crlf(&meta_part[meta_start..])).unwrap();
                let media_part = parts[2];
                let media_start = find(media_part, b"\r\n\r\n").unwrap() + 4;
                let media = trim_crlf(&media_part[media_start..]).to_vec();
                assert_eq!(meta["parents"], json!(["appDataFolder"]));
                db.next += 1;
                let id = format!("f{}", db.next);
                db.files.push((
                    id.clone(),
                    meta["name"].as_str().unwrap().to_string(),
                    media,
                ));
                json(json!({ "id": id }))
            } else if let Some(id) = path.strip_prefix("/upload/files/") {
                if let Some(file) = db.files.iter_mut().find(|(fid, _, _)| fid == id) {
                    file.2 = body;
                }
                json(json!({}))
            } else {
                tiny_http::Response::from_string("unknown").with_status_code(404)
            };
            drop(db);
            let _ = request.respond(response);
        }
    });
    let endpoints = DriveEndpoints {
        token: format!("{base}/token"),
        api: format!("{base}/api"),
        upload: format!("{base}/upload"),
    };
    (endpoints, state)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn split<'a>(data: &'a [u8], sep: &[u8]) -> Vec<&'a [u8]> {
    let mut out = Vec::new();
    let mut rest = data;
    while let Some(i) = find(rest, sep) {
        out.push(&rest[..i]);
        rest = &rest[i + sep.len()..];
    }
    out.push(rest);
    out
}

fn trim_crlf(b: &[u8]) -> &[u8] {
    b.strip_suffix(b"\r\n").unwrap_or(b)
}

fn creds() -> DriveCredentials {
    DriveCredentials {
        client_id: "client".into(),
        client_secret: "secret".into(),
        refresh_token: "refresh-1".into(),
    }
}

#[test]
fn drive_store_puts_lists_gets_and_replaces_snapshots() {
    let (endpoints, files) = fake_drive();
    let drive = DriveStore::with_endpoints(creds(), endpoints);
    let mac = DeviceId::from("mac-1");
    drive.put(&mac, 1, b"one").unwrap();
    drive.put(&mac, 2, b"two").unwrap();
    drive.put(&DeviceId::from("phone-1"), 1, b"p1").unwrap();
    assert_eq!(
        drive.devices().unwrap(),
        vec![mac.clone(), DeviceId::from("phone-1")]
    );
    assert_eq!(drive.list(&mac, 0).unwrap(), vec![1, 2]);
    assert_eq!(drive.list(&mac, 1).unwrap(), vec![2]);
    assert_eq!(drive.get(&mac, 2).unwrap(), b"two");

    assert_eq!(drive.snapshot(&mac).unwrap(), None);
    drive.put_snapshot(&mac, b"snap-1").unwrap();
    drive.put_snapshot(&mac, b"snap-2").unwrap();
    assert_eq!(drive.snapshot(&mac).unwrap().unwrap(), b"snap-2");
    assert_eq!(
        files
            .lock()
            .unwrap()
            .files
            .iter()
            .filter(|f| f.1.starts_with("s."))
            .count(),
        1,
        "replaced, not duplicated"
    );

    // A device id that could break the query is refused before any request.
    assert!(drive
        .list(&DeviceId::from("x' or name contains '"), 0)
        .is_err());
}

#[test]
fn sign_in_uses_pkce_and_returns_a_refresh_token() {
    let (endpoints, _) = fake_drive();
    let creds = authorize_drive(
        "client",
        "secret",
        &endpoints,
        Some("http://consent.invalid/auth"),
        |url| {
            // Play the browser: Google redirects back with a code and the state.
            assert!(url.contains("code_challenge_method=S256"));
            assert!(url.contains("scope=https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fdrive.appdata"));
            let query = url.split_once('?').unwrap().1;
            let redirect = param(query, "redirect_uri").unwrap();
            let state = param(query, "state").unwrap();
            thread::spawn(move || {
                ureq::get(&format!("{redirect}/?state={state}&code=auth-code"))
                    .call()
                    .unwrap();
            });
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(creds.refresh_token, "refresh-1");
}

fn schema() -> PluginSchema {
    serde_json::from_value(json!({"version": 1, "tables": [
        {"name": "tasks", "columns": [{"name": "title", "type": "text"}]}
    ]}))
    .unwrap()
}

fn stores(dir: &Path, device: &DeviceId) -> BTreeMap<String, Store> {
    std::fs::create_dir_all(dir).unwrap();
    let cfg = StoreConfig {
        space: "personal".into(),
        plugin: "todo".into(),
        device: device.clone(),
        user: "owner".into(),
    };
    let store = Store::open(
        dir.join("todo.db"),
        cfg,
        schema(),
        Box::new(ManualTime::new(1_000)),
    )
    .unwrap();
    [("todo".to_string(), store)].into_iter().collect()
}

#[test]
fn phone_syncs_through_drive_while_the_mac_is_unreachable() {
    let root = TempDir::new().unwrap();
    let (endpoints, _) = fake_drive();
    let key = SpaceKey::generate();
    let mac_keys = DeviceKeys::generate("mac".into());
    let phone_keys = DeviceKeys::generate("phone".into());
    let roster: Roster = [&mac_keys, &phone_keys]
        .iter()
        .map(|k| (k.device.clone(), k.public("d")))
        .collect();
    let mut mac_stores = stores(&root.path().join("mac"), &mac_keys.device);
    let mut phone_stores = stores(&root.path().join("phone"), &phone_keys.device);
    let mut mac = SpaceSync::open(
        "personal",
        mac_keys.clone(),
        key.clone(),
        roster.clone(),
        &root.path().join("mac/sync"),
    )
    .unwrap();
    let mut phone = SpaceSync::open(
        "personal",
        phone_keys.clone(),
        key,
        roster,
        &root.path().join("phone/sync"),
    )
    .unwrap();

    let mac_transport = MultiStore::new(vec![
        (
            "mac folder".into(),
            Box::new(FolderStore::new(root.path().join("relay"))),
        ),
        (
            "drive".into(),
            Box::new(DriveStore::with_endpoints(creds(), endpoints.clone())),
        ),
    ]);
    // The Mac is asleep: its address does not answer.
    let phone_transport = MultiStore::new(vec![
        (
            "mac".into(),
            Box::new(
                HttpStore::new(
                    "127.0.0.1:9",
                    "personal",
                    &DeviceKeys::generate("test".into()),
                    &SpaceKey::generate(),
                )
                .unwrap(),
            ),
        ),
        (
            "drive".into(),
            Box::new(DriveStore::with_endpoints(creds(), endpoints)),
        ),
    ]);

    let values = json!({"title": "added on the phone"})
        .as_object()
        .cloned()
        .unwrap();
    phone_stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.insert("tasks", &values))
        .unwrap();
    let report = phone.sync(&mut phone_stores, &phone_transport).unwrap();
    assert_eq!(report.pushed, Some(1));
    assert!(
        phone_transport.errors().contains_key("mac"),
        "the dead host is reported"
    );

    // Later the Mac wakes up and finds the phone's batch in Drive.
    let values = json!({"title": "added on the mac"})
        .as_object()
        .cloned()
        .unwrap();
    mac_stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.insert("tasks", &values))
        .unwrap();
    assert_eq!(
        mac.sync(&mut mac_stores, &mac_transport).unwrap().batches,
        1
    );
    assert_eq!(
        phone
            .sync(&mut phone_stores, &phone_transport)
            .unwrap()
            .batches,
        1
    );

    let dump = |s: &BTreeMap<String, Store>| json!(s["todo"].dump().unwrap());
    assert_eq!(dump(&mac_stores), dump(&phone_stores));
    assert_eq!(dump(&mac_stores)["tasks"].as_array().unwrap().len(), 2);
}
