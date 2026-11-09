//! Pairing and syncing over HTTP on loopback: the host serves its batch
//! folder, the joiner pairs with a code and then syncs through the host.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tempfile::TempDir;
use terrarium_core::{DeviceId, ManualTime, PluginSchema, Store, StoreConfig};
use terrarium_sync::{
    pair_with, pairing, BatchStore, DeviceKeys, Envelope, FolderStore, HttpStore, Payload,
    PeerServer, Relay, Roster, SpaceKey, SpaceSync,
};

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

struct Host {
    relay: Arc<Relay>,
    server: PeerServer,
    saved: Arc<Mutex<Option<Roster>>>,
    keys: DeviceKeys,
    key: SpaceKey,
    folder: std::path::PathBuf,
}

fn host(root: &TempDir) -> Host {
    let keys = DeviceKeys::generate("mac".into());
    let key = SpaceKey::generate();
    let roster: Roster = [(keys.device.clone(), keys.public("Mac"))]
        .into_iter()
        .collect();
    let folder = root.path().join("relay");
    let saved = Arc::new(Mutex::new(None));
    let hook = saved.clone();
    let relay = Arc::new(Relay::new(
        "personal",
        key.clone(),
        FolderStore::new(&folder),
        roster,
        Box::new(move |r: &Roster| {
            *hook.lock().unwrap() = Some(r.clone());
            Ok(())
        }),
    ));
    let server = PeerServer::start(&["127.0.0.1:0".parse().unwrap()], relay.clone()).unwrap();
    Host {
        relay,
        server,
        saved,
        keys,
        key,
        folder,
    }
}

impl Host {
    fn address(&self) -> String {
        self.server.addrs()[0].to_string()
    }
}

#[test]
fn pair_then_sync_through_the_host() {
    let root = TempDir::new().unwrap();
    let host = host(&root);
    let (code, ttl) = host.relay.start_pairing();
    assert!(ttl.as_secs() > 200);

    let phone_keys = DeviceKeys::generate("phone".into());
    let joined = pair_with(
        &host.address(),
        &pairing::display_code(&code),
        &phone_keys,
        "Phone",
    )
    .unwrap();
    assert_eq!(joined.key, host.key);
    assert_eq!(joined.space, "personal");
    assert!(joined.roster.contains_key(&phone_keys.device));
    assert_eq!(
        host.saved.lock().unwrap().as_ref().unwrap().len(),
        2,
        "host persisted the new roster"
    );
    assert!(!host.relay.pairing_active(), "code is single use");

    // Mac syncs against its own folder; the phone through HTTP.
    let mut mac_stores = stores(&root.path().join("mac"), &host.keys.device);
    let mut mac = SpaceSync::open(
        "personal",
        host.keys.clone(),
        host.key.clone(),
        host.relay.roster(),
        &root.path().join("mac/sync"),
    )
    .unwrap();
    let mut phone_stores = stores(&root.path().join("phone"), &phone_keys.device);
    let mut phone = SpaceSync::open(
        "personal",
        phone_keys.clone(),
        joined.key,
        joined.roster,
        &root.path().join("phone/sync"),
    )
    .unwrap();
    let folder = FolderStore::new(&host.folder);
    let http = HttpStore::new(&host.address(), "personal", &phone_keys, &host.key).unwrap();

    let values = json!({"title": "from the phone"})
        .as_object()
        .cloned()
        .unwrap();
    phone_stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.insert("tasks", &values))
        .unwrap();
    let values = json!({"title": "from the mac"})
        .as_object()
        .cloned()
        .unwrap();
    mac_stores
        .get_mut("todo")
        .unwrap()
        .write(|w| w.insert("tasks", &values))
        .unwrap();

    assert_eq!(
        phone.sync(&mut phone_stores, &http).unwrap().pushed,
        Some(1)
    );
    let report = mac.sync(&mut mac_stores, &folder).unwrap();
    assert_eq!(report.batches, 1);
    let report = phone.sync(&mut phone_stores, &http).unwrap();
    assert_eq!(report.batches, 1);

    let dump = |s: &BTreeMap<String, Store>| json!(s["todo"].dump().unwrap());
    assert_eq!(dump(&mac_stores), dump(&phone_stores));
    assert_eq!(dump(&mac_stores)["tasks"].as_array().unwrap().len(), 2);
    assert_eq!(http.roster().unwrap().len(), 2);
}

#[test]
fn wrong_or_reused_codes_do_not_pair() {
    let root = TempDir::new().unwrap();
    let host = host(&root);
    let phone = DeviceKeys::generate("phone".into());

    let err = pair_with(&host.address(), "AAAAA-AAAAA", &phone, "Phone")
        .err()
        .unwrap();
    assert!(err.to_string().contains("403"), "{err}");

    let (code, _) = host.relay.start_pairing();
    let err = pair_with(&host.address(), "AAAAA-AAAAA", &phone, "Phone")
        .err()
        .unwrap();
    assert!(err.to_string().contains("403"), "{err}");
    pair_with(&host.address(), &code, &phone, "Phone").unwrap();

    let other = DeviceKeys::generate("tablet".into());
    let err = pair_with(&host.address(), &code, &other, "Tablet")
        .err()
        .unwrap();
    assert!(err.to_string().contains("403"), "{err}");
    assert_eq!(host.relay.roster().len(), 2);
}

#[test]
fn host_refuses_batches_it_cannot_verify() {
    let root = TempDir::new().unwrap();
    let host = host(&root);
    let http = HttpStore::new(&host.address(), "personal", &host.keys, &host.key).unwrap();
    let empty = Payload {
        batches: vec![],
        received: Default::default(),
    };

    // Not paired.
    let intruder = DeviceKeys::generate("intruder".into());
    let sealed = Envelope::seal("personal", 1, &empty, &host.key, &intruder).unwrap();
    let err = http
        .put(&intruder.device, 1, &sealed.to_bytes().unwrap())
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // Paired, but filed under another device's folder.
    let sealed = Envelope::seal("personal", 1, &empty, &host.key, &host.keys).unwrap();
    let err = http
        .put(&DeviceId::from("phone"), 1, &sealed.to_bytes().unwrap())
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // Garbage.
    let err = http.put(&host.keys.device, 2, b"not a batch").unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    // Path traversal in the device id never reaches the folder.
    let err = http.list(&DeviceId::from("..%2F.."), 0).unwrap_err();
    assert!(err.to_string().contains("safe name"), "{err}");

    assert!(FolderStore::new(&host.folder).devices().unwrap().is_empty());
}

#[test]
fn unreachable_host_is_a_clear_error() {
    let http = HttpStore::new(
        "127.0.0.1:9",
        "personal",
        &DeviceKeys::generate("test".into()),
        &SpaceKey::generate(),
    )
    .unwrap();
    let err = http.devices().unwrap_err();
    assert!(err.to_string().contains("not reachable"), "{err}");
}

#[test]
fn oversized_declared_bodies_are_rejected_before_reading_or_allocating_them() {
    use std::io::{Read, Write};
    let root = TempDir::new().unwrap();
    let host = host(&root);
    let mut socket = std::net::TcpStream::connect(host.address()).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    socket.write_all(b"POST /v1/pair HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000000\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(host.relay.roster().len() == 1);

    // No Content-Length: the streaming limit must still reject an oversized body.
    let mut chunked = std::net::TcpStream::connect(host.address()).unwrap();
    chunked
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .unwrap();
    chunked.write_all(b"POST /v1/pair HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10001\r\n").unwrap();
    chunked.write_all(&vec![b'x'; 64 * 1024 + 1]).unwrap();
    chunked.write_all(b"\r\n0\r\n\r\n").unwrap();
    let mut response = String::new();
    chunked.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    assert!(host.relay.roster().len() == 1);
}

#[test]
fn browser_requests_are_refused_and_slow_connections_do_not_prevent_shutdown() {
    use std::io::Write;
    let root = TempDir::new().unwrap();
    let host = host(&root);
    let error = ureq::get(&format!("http://{}/v1/roster", host.address()))
        .set("Origin", "https://untrusted.example")
        .call()
        .unwrap_err();
    assert!(matches!(error, ureq::Error::Status(403, _)));
    let mut slow = std::net::TcpStream::connect(host.address()).unwrap();
    slow.write_all(b"POST /v1/pair HTTP/1.1\r\nHost: localhost\r\nContent-Length: 12\r\n\r\n")
        .unwrap();
    let start = std::time::Instant::now();
    drop(host);
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn wildcard_listeners_and_unapproved_device_paths_are_rejected() {
    let root = TempDir::new().unwrap();
    let host = host(&root);
    assert!(PeerServer::start(&["0.0.0.0:0".parse().unwrap()], host.relay.clone()).is_err());
    let http = HttpStore::new(&host.address(), "personal", &host.keys, &host.key).unwrap();
    assert!(http.list(&"../roster".into(), 0).is_err());
    host.relay
        .set_config("drive", Some(b"sealed-test-bytes".to_vec()));
    assert_eq!(http.config("drive").unwrap().unwrap(), b"sealed-test-bytes");
}

#[test]
fn peer_redirects_are_not_followed() {
    use std::io::{Read, Write};
    let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let redirect = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = redirect.local_addr().unwrap();
    let location = target.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = redirect.accept().unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
            .unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            headers.push(byte[0]);
            assert!(headers.len() <= 4096);
        }
        write!(stream, "HTTP/1.1 302 Found\r\nLocation: http://{location}/v1/devices\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let http = HttpStore::new(
        &address.to_string(),
        "personal",
        &DeviceKeys::generate("test".into()),
        &SpaceKey::generate(),
    )
    .unwrap();
    assert!(http.devices().unwrap_err().to_string().contains("302"));
    server.join().unwrap();
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}
