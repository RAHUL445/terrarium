//! A device that hosts the space's batch folder for its peers: serves it
//! over HTTP on the tailnet and runs pairing.
//!
//! Reads and writes require a live roster member and a single-use challenge.
//! Responses authenticate against the existing space key. Pairing remains
//! code-authenticated. Encrypted batches still verify their device signature.
//!
//! Routes (all under `/v1`):
//!   GET  /devices                     JSON array of device ids
//!   GET  /batches/<device>?after=<n>  JSON array of sequence numbers
//!   GET  /batches/<device>/<seq>      batch bytes
//!   PUT  /batches/<device>/<seq>      store a batch (verified)
//!   GET  /snapshots/<device>          latest snapshot bytes (404 if none)
//!   PUT  /snapshots/<device>          store a snapshot (verified)
//!   GET  /config/<name>               sealed space setting (404 if none)
//!   GET  /roster                      JSON roster
//!   GET  /challenge                   single-use request nonce
//!   POST /pair                        JSON PairRequest -> PairResponse

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    Method, Request, Response,
};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use terrarium_core::DeviceId;

use crate::envelope::Envelope;
use crate::error::{Result, SyncError};
use crate::keys::{Roster, SpaceKey};
use crate::pairing::{PairRequest, PairingSession};
use crate::transport::{BatchStore, FolderStore};

/// Largest batch accepted, and largest pairing request.
const MAX_BATCH_BODY: usize = 32 * 1024 * 1024;
const MAX_SNAPSHOT_BODY: usize = 128 * 1024 * 1024;
const MAX_PAIR_BODY: usize = 64 * 1024;

type RosterHook = Box<dyn Fn(&Roster) -> Result<()> + Send + Sync>;

/// The host side of the space: folder, roster and pairing.
pub struct Relay {
    space: String,
    key: SpaceKey,
    store: FolderStore,
    roster: Mutex<Roster>,
    challenges: Mutex<crate::http_auth::Challenges>,
    pairing: Mutex<Option<PairingSession>>,
    on_roster: RosterHook,
    /// Sealed settings (kind `config`) peers fetch, e.g. Drive credentials.
    config: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
}

impl Relay {
    /// `on_roster` must durably save membership before it is published live.
    /// It runs under the roster lock and must not call back into the relay.
    pub fn new(
        space: &str,
        key: SpaceKey,
        store: FolderStore,
        roster: Roster,
        on_roster: RosterHook,
    ) -> Relay {
        Relay {
            space: space.to_string(),
            key,
            store,
            roster: Mutex::new(roster),
            challenges: Mutex::new(Default::default()),
            pairing: Mutex::new(None),
            on_roster,
            config: Mutex::new(Default::default()),
        }
    }

    /// Publishes (or with `None`, withdraws) a sealed setting for peers.
    pub fn set_config(&self, name: &str, sealed: Option<Vec<u8>>) {
        let mut config = self.config.lock().unwrap();
        match sealed {
            Some(bytes) => config.insert(name.to_string(), bytes),
            None => config.remove(name),
        };
    }

    pub fn roster(&self) -> Roster {
        self.roster.lock().unwrap().clone()
    }

    /// Host-local removal, persisted before it affects authorization.
    /// This blocks future peer requests; it cannot erase data or rotate keys.
    pub fn remove_device(&self, device: &DeviceId, host: &DeviceId) -> Result<()> {
        if device == host {
            return Err(SyncError::Pairing("cannot remove this host device".into()));
        }
        let mut slot = self.pairing.lock().unwrap();
        let mut roster = self.roster.lock().unwrap();
        if !roster.contains_key(host) || !roster.contains_key(device) {
            return Err(SyncError::Pairing("device is not in this space".into()));
        }
        let mut next = roster.clone();
        next.remove(device);
        (self.on_roster)(&next)?;
        *roster = next;
        // A previously shown code cannot immediately re-authorize the device.
        *slot = None;
        Ok(())
    }

    /// Starts (or restarts) pairing; returns the code to show.
    pub fn start_pairing(&self) -> (String, Duration) {
        let session = PairingSession::new();
        let shown = (session.code().to_string(), session.remaining());
        *self.pairing.lock().unwrap() = Some(session);
        shown
    }

    pub fn cancel_pairing(&self) {
        *self.pairing.lock().unwrap() = None;
    }

    pub fn pairing_active(&self) -> bool {
        self.pairing
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|s| !s.remaining().is_zero())
    }

    fn pair(&self, request: &PairRequest) -> Result<crate::pairing::PairResponse> {
        let mut slot = self.pairing.lock().unwrap();
        let session = slot
            .as_mut()
            .ok_or_else(|| SyncError::Pairing("no pairing in progress on this device".into()))?;
        if let Err(e) = session.verify(request) {
            if session.remaining().is_zero() || e.to_string().contains("too many") {
                *slot = None;
            }
            return Err(e);
        }
        crate::transport::check_device(&request.device.device)?;
        let signing = ed25519_dalek::VerifyingKey::from_bytes(&request.device.signing)
            .map_err(|_| SyncError::Pairing("invalid device signing key".into()))?;
        if signing.is_weak() {
            return Err(SyncError::Pairing("invalid device signing key".into()));
        }
        let response = {
            let mut roster = self.roster.lock().unwrap();
            if let Some(existing) = roster.get(&request.device.device) {
                if existing.signing != request.device.signing
                    || existing.exchange != request.device.exchange
                {
                    return Err(SyncError::Pairing(
                        "a different device already uses this id".into(),
                    ));
                }
            }
            let mut next = roster.clone();
            next.insert(request.device.device.clone(), request.device.clone());
            // Wrapping can fail: publish membership only after response construction.
            let response = session.respond(&self.space, &next, &self.key, &request.device)?;
            (self.on_roster)(&next)?;
            *roster = next.clone();
            response
        };
        *slot = None;
        drop(slot);
        Ok(response)
    }

    fn accept(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()> {
        let envelope = Envelope::from_bytes(bytes).map_err(|_| SyncError::Rejected {
            device: device.to_string(),
            seq,
            reason: "not a batch".into(),
        })?;
        if &envelope.device != device || envelope.seq != seq {
            return Err(SyncError::Rejected {
                device: device.to_string(),
                seq,
                reason: "batch does not match its path".into(),
            });
        }
        if envelope.kind != crate::envelope::Kind::Batch {
            return Err(SyncError::Rejected {
                device: device.to_string(),
                seq,
                reason: "not a batch".into(),
            });
        }
        envelope.open(&self.space, &self.key, &self.roster())?;
        self.store.put(device, seq, bytes)
    }

    fn accept_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()> {
        let reject = |reason: &str| SyncError::Rejected {
            device: device.to_string(),
            seq: 0,
            reason: reason.into(),
        };
        let envelope = Envelope::from_bytes(bytes).map_err(|_| reject("not a snapshot"))?;
        if &envelope.device != device || envelope.kind != crate::envelope::Kind::Snapshot {
            return Err(reject("snapshot does not match its path"));
        }
        envelope.open(&self.space, &self.key, &self.roster())?;
        self.store.put_snapshot(device, bytes)
    }
}

type CallbackHandler = dyn Fn(&Method, &str) -> PeerResponse + Send + Sync;

#[derive(Clone)]
enum Endpoint {
    Peer(Arc<Relay>),
    Callback(Arc<CallbackHandler>),
}

pub struct PeerServer {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    addrs: Vec<SocketAddr>,
}

impl PeerServer {
    /// Listens on each address (e.g. the tailnet address and loopback).
    pub fn start(addrs: &[SocketAddr], relay: Arc<Relay>) -> Result<PeerServer> {
        Self::start_endpoint(addrs, Endpoint::Peer(relay))
    }

    pub(crate) fn callback(
        handler: impl Fn(&Method, &str) -> PeerResponse + Send + Sync + 'static,
    ) -> Result<PeerServer> {
        Self::start_endpoint(
            &[SocketAddr::from(([127, 0, 0, 1], 0))],
            Endpoint::Callback(Arc::new(handler)),
        )
    }

    fn start_endpoint(addrs: &[SocketAddr], endpoint: Endpoint) -> Result<PeerServer> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut listeners = Vec::new();
        let mut bound = Vec::new();
        for addr in addrs {
            let private = match addr.ip() {
                std::net::IpAddr::V4(ip) => {
                    ip.is_loopback()
                        || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
                }
                std::net::IpAddr::V6(ip) => ip.is_loopback(),
            };
            if !private {
                return Err(SyncError::Transport(
                    "peer listeners require loopback or a Tailscale IPv4 address".into(),
                ));
            }
        }
        // Bind everything before launching: partial startup cannot leak workers.
        for addr in addrs {
            let listener = std::net::TcpListener::bind(addr)
                .map_err(|e| SyncError::Transport(format!("listen: {e}")))?;
            listener
                .set_nonblocking(true)
                .map_err(|e| SyncError::Transport(e.to_string()))?;
            bound.push(
                listener
                    .local_addr()
                    .map_err(|e| SyncError::Transport(e.to_string()))?,
            );
            listeners.push(listener);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            runtime.block_on(async move {
                let connections = Arc::new(tokio::sync::Semaphore::new(8));
                let requests = Arc::new(tokio::sync::Semaphore::new(1));
                for listener in listeners {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        continue;
                    };
                    let (endpoint, connections, requests) =
                        (endpoint.clone(), connections.clone(), requests.clone());
                    tokio::spawn(async move {
                        while let Ok((stream, _)) = listener.accept().await {
                            let Ok(permit) = connections.clone().try_acquire_owned() else {
                                continue;
                            };
                            let (endpoint, requests) = (endpoint.clone(), requests.clone());
                            tokio::spawn(async move {
                                let _permit = permit;
                                let service = hyper::service::service_fn(move |request| {
                                    handle(endpoint.clone(), requests.clone(), request)
                                });
                                let mut http = hyper::server::conn::http1::Builder::new();
                                http.keep_alive(false)
                                    .max_buf_size(16 * 1024)
                                    .max_headers(64);
                                let _ = tokio::time::timeout(
                                    Duration::from_secs(15),
                                    http.serve_connection(TokioIo::new(stream), service),
                                )
                                .await;
                            });
                        }
                    });
                }
                while !stopped.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                // Dropping the runtime aborts pending connections, including slow bodies.
            })
        });
        Ok(PeerServer {
            stop,
            threads: vec![thread],
            addrs: bound,
        })
    }

    /// Bound addresses (useful when port 0 was requested).
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }
}

impl Drop for PeerServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

pub(crate) type PeerResponse = Response<Full<Bytes>>;

fn reply(status: u16, body: Vec<u8>, json: bool) -> PeerResponse {
    Response::builder()
        .status(status)
        .header(
            "Content-Type",
            if json {
                "application/json"
            } else {
                "application/octet-stream"
            },
        )
        .header("Cache-Control", "no-store")
        .header("Connection", "close")
        .body(Full::new(Bytes::from(body)))
        .expect("static response headers")
}

fn reply_error(status: u16, message: &str) -> PeerResponse {
    reply(
        status,
        serde_json::json!({"error":message})
            .to_string()
            .into_bytes(),
        true,
    )
}

pub(crate) fn callback_reply(status: u16, text: &str) -> PeerResponse {
    let mut response = reply(status, text.as_bytes().to_vec(), false);
    response.headers_mut().insert(
        "Content-Type",
        hyper::header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

async fn handle(
    endpoint: Endpoint,
    requests: Arc<tokio::sync::Semaphore>,
    request: Request<Incoming>,
) -> std::result::Result<PeerResponse, Infallible> {
    if matches!(endpoint, Endpoint::Peer(_)) && request.headers().contains_key("Origin") {
        return Ok(reply_error(
            403,
            "browser-origin requests are not supported",
        ));
    }
    let Ok(_permit) = requests.try_acquire_owned() else {
        return Ok(reply_error(503, "peer server is busy"));
    };
    let method = request.method().clone();
    let url = request.uri().to_string();
    let parts: Vec<_> = request.uri().path().trim_matches('/').split('/').collect();
    let limit = match (&endpoint, &method, parts.as_slice()) {
        (Endpoint::Peer(_), &Method::POST, ["v1", "pair"]) => MAX_PAIR_BODY,
        (Endpoint::Peer(_), &Method::PUT, ["v1", "batches", _, _]) => MAX_BATCH_BODY,
        (Endpoint::Peer(_), &Method::PUT, ["v1", "snapshots", _]) => MAX_SNAPSHOT_BODY,
        _ => 0,
    };
    if request
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|len| len > limit)
    {
        return Ok(reply_error(413, "request body too large"));
    }
    // Pairing proves the one-time code; challenges reveal no membership metadata.
    let proof = if let Endpoint::Peer(relay) = &endpoint {
        if method == Method::GET && url == "/v1/challenge" {
            if request.headers().contains_key("Transfer-Encoding") {
                return Ok(reply_error(400, "challenge requests cannot have a body"));
            }
            let nonce = relay.challenges.lock().unwrap().issue();
            return Ok(match nonce {
                Some(nonce) => reply(
                    200,
                    serde_json::json!({"nonce":nonce}).to_string().into_bytes(),
                    true,
                ),
                None => reply_error(503, "challenge capacity reached; retry later"),
            });
        }
        if method == Method::POST && url == "/v1/pair" {
            None
        } else {
            let Some(proof) = request
                .headers()
                .get(crate::http_auth::AUTH_HEADER)
                .and_then(|h| h.to_str().ok())
                .and_then(crate::http_auth::Proof::parse)
            else {
                return Ok(reply_error(
                    401,
                    "authenticated peer protocol required; update both devices",
                ));
            };
            if !relay.challenges.lock().unwrap().authorize(
                &proof,
                &relay.roster(),
                &relay.space,
                method.as_str(),
                &url,
            ) {
                return Ok(reply_error(401, "peer authentication failed"));
            }
            Some(proof)
        }
    } else {
        None
    };
    let body = match Limited::new(request.into_body(), limit).collect().await {
        Ok(body) => body.to_bytes(),
        Err(_) => return Ok(reply_error(413, "request body invalid or too large")),
    };
    Ok(match endpoint {
        Endpoint::Peer(relay) => {
            if let Some(proof) = proof {
                if !proof.matches_body(&body) {
                    return Ok(reply_error(401, "request body authentication failed"));
                }
                let mut response = dispatch(&relay, method.clone(), &url, &body);
                let bytes = response
                    .body()
                    .clone()
                    .collect()
                    .await
                    .expect("infallible full body")
                    .to_bytes();
                let tag = proof.response_tag(
                    &relay.key,
                    &relay.space,
                    method.as_str(),
                    &url,
                    response.status().as_u16(),
                    &bytes,
                );
                response.headers_mut().insert(
                    crate::http_auth::RESPONSE_HEADER,
                    tag.parse().expect("base64 header"),
                );
                response
            } else {
                dispatch(&relay, method, &url, &body)
            }
        }
        Endpoint::Callback(callback) => callback(&method, &url),
    })
}

fn dispatch(relay: &Relay, method: Method, url: &str, body: &[u8]) -> PeerResponse {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (method, parts.as_slice()) {
        (Method::GET, ["v1", "devices"]) => match relay.store.devices() {
            Ok(devices) => reply(200, serde_json::to_vec(&devices).unwrap_or_default(), true),
            Err(e) => reply_error(500, &e.to_string()),
        },
        (Method::GET, ["v1", "config", name]) => {
            match relay.config.lock().unwrap().get(*name).cloned() {
                Some(bytes) => reply(200, bytes, false),
                None => reply_error(404, "no such config"),
            }
        }
        (Method::GET, ["v1", "roster"]) => reply(
            200,
            serde_json::to_vec(&relay.roster()).unwrap_or_default(),
            true,
        ),
        (Method::GET, ["v1", "batches", device]) => {
            let after = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("after="))
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            match relay.store.list(&DeviceId::from(*device), after) {
                Ok(seqs) => reply(200, serde_json::to_vec(&seqs).unwrap_or_default(), true),
                Err(e) => reply_error(400, &e.to_string()),
            }
        }
        (Method::GET, ["v1", "batches", device, seq]) => {
            let Ok(seq) = seq.parse() else {
                return reply_error(400, "bad sequence number");
            };
            match relay.store.get(&DeviceId::from(*device), seq) {
                Ok(bytes) => reply(200, bytes, false),
                Err(_) => reply_error(404, "no such batch"),
            }
        }
        (Method::PUT, ["v1", "batches", device, seq]) => {
            let Ok(seq) = seq.parse() else {
                return reply_error(400, "bad sequence number");
            };
            match relay.accept(&DeviceId::from(*device), seq, body) {
                Ok(()) => reply(204, Vec::new(), false),
                Err(e @ SyncError::Rejected { .. }) => reply_error(403, &e.to_string()),
                Err(e) => reply_error(500, &e.to_string()),
            }
        }
        (Method::GET, ["v1", "snapshots", device]) => {
            match relay.store.snapshot(&DeviceId::from(*device)) {
                Ok(Some(bytes)) => reply(200, bytes, false),
                Ok(None) => reply_error(404, "no snapshot"),
                Err(e) => reply_error(400, &e.to_string()),
            }
        }
        (Method::PUT, ["v1", "snapshots", device]) => {
            match relay.accept_snapshot(&DeviceId::from(*device), body) {
                Ok(()) => reply(204, Vec::new(), false),
                Err(e @ SyncError::Rejected { .. }) => reply_error(403, &e.to_string()),
                Err(e) => reply_error(500, &e.to_string()),
            }
        }
        (Method::POST, ["v1", "pair"]) => {
            let Ok(pair) = serde_json::from_slice::<PairRequest>(body) else {
                return reply_error(400, "malformed pairing request");
            };
            match relay.pair(&pair) {
                Ok(response) => reply(200, serde_json::to_vec(&response).unwrap_or_default(), true),
                Err(e) => reply_error(403, &e.to_string()),
            }
        }
        _ => reply_error(404, "not found"),
    }
}

#[cfg(test)]
mod auth_tests {
    use super::*;
    use crate::{
        http_auth::{Proof, AUTH_HEADER, RESPONSE_HEADER},
        DeviceKeys, HttpStore,
    };
    fn setup() -> (
        tempfile::TempDir,
        DeviceKeys,
        SpaceKey,
        Arc<Relay>,
        PeerServer,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let keys = DeviceKeys::generate("host".into());
        let key = SpaceKey::generate();
        let roster = [(keys.device.clone(), keys.public("Host"))]
            .into_iter()
            .collect();
        let relay = Arc::new(Relay::new(
            "space",
            key.clone(),
            FolderStore::new(dir.path()),
            roster,
            Box::new(|_| Ok(())),
        ));
        let server = PeerServer::start(&["127.0.0.1:0".parse().unwrap()], relay.clone()).unwrap();
        let address = server.addrs()[0].to_string();
        (dir, keys, key, relay, server, address)
    }
    #[test]
    fn membership_is_persisted_before_publication_and_failure_preserves_pairing() {
        let dir = tempfile::tempdir().unwrap();
        let host = DeviceKeys::generate("host".into());
        let phone = DeviceKeys::generate("phone".into());
        let original: Roster = [
            (host.device.clone(), host.public("Host")),
            (phone.device.clone(), phone.public("Phone")),
        ]
        .into_iter()
        .collect();
        let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = fail.clone();
        let saved = dir.path().join("roster.json");
        std::fs::write(&saved, serde_json::to_vec(&original).unwrap()).unwrap();
        let path = saved.clone();
        let relay = Relay::new(
            "space",
            SpaceKey::generate(),
            FolderStore::new(dir.path().join("relay")),
            original.clone(),
            Box::new(move |roster| {
                if gate.load(Ordering::SeqCst) {
                    return Err(SyncError::Transport("synthetic persistence failure".into()));
                }
                std::fs::write(&path, serde_json::to_vec(roster)?)
                    .map_err(|e| SyncError::io(&path, e))
            }),
        );
        relay.start_pairing();
        assert!(relay.remove_device(&phone.device, &host.device).is_err());
        assert_eq!(relay.roster(), original);
        assert!(relay.pairing_active());
        assert_eq!(
            serde_json::from_slice::<Roster>(&std::fs::read(&saved).unwrap()).unwrap(),
            original
        );
        // Failed pairing persistence cannot authorize the candidate either.
        let candidate = DeviceKeys::generate("candidate".into());
        let (code, _) = relay.start_pairing();
        let request = crate::pairing::request(&code, &candidate.public("Candidate")).unwrap();
        assert!(relay.pair(&request).is_err());
        assert_eq!(relay.roster(), original);
        fail.store(false, Ordering::SeqCst);
        assert!(relay.remove_device(&host.device, &host.device).is_err());
        relay.remove_device(&phone.device, &host.device).unwrap();
        assert!(!relay.pairing_active());
        let persisted: Roster = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
        assert_eq!(persisted, relay.roster());
        assert!(!persisted.contains_key(&phone.device));
        let reopened = Relay::new(
            "space",
            SpaceKey::generate(),
            FolderStore::new(dir.path().join("relay")),
            persisted,
            Box::new(|_| Ok(())),
        );
        assert!(!reopened.roster().contains_key(&phone.device));
    }

    fn proof(address: &str, keys: &DeviceKeys, method: &str, path: &str, body: &[u8]) -> Proof {
        let response: serde_json::Value = ureq::get(&format!("http://{address}/v1/challenge"))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        Proof::create(
            keys,
            "space",
            method,
            path,
            response["nonce"].as_str().unwrap().into(),
            body,
        )
    }
    fn unauthorized(result: std::result::Result<ureq::Response, ureq::Error>) {
        assert!(matches!(result, Err(ureq::Error::Status(401, _))));
    }
    #[test]
    fn reads_require_membership_and_signed_requests_cannot_be_replayed_or_retargeted() {
        let (_dir, keys, key, relay, _server, address) = setup();
        for path in [
            "/v1/roster",
            "/v1/devices",
            "/v1/config/drive",
            "/v1/snapshots/host",
            "/v1/batches/host?after=0",
        ] {
            unauthorized(ureq::get(&format!("http://{address}{path}")).call());
        }
        let p = proof(&address, &keys, "GET", "/v1/roster", b"");
        unauthorized(
            ureq::get(&format!("http://{address}/v1/devices"))
                .set(AUTH_HEADER, &p.header())
                .call(),
        );
        let response = ureq::get(&format!("http://{address}/v1/roster"))
            .set(AUTH_HEADER, &p.header())
            .call()
            .unwrap();
        let tag = response.header(RESPONSE_HEADER).unwrap().to_string();
        let body = response.into_string().unwrap();
        assert!(p.verify_response(
            &tag,
            &key,
            ("space", "GET", "/v1/roster"),
            200,
            body.as_bytes()
        ));
        unauthorized(
            ureq::get(&format!("http://{address}/v1/roster"))
                .set(AUTH_HEADER, &p.header())
                .call(),
        );
        let pending = proof(&address, &keys, "GET", "/v1/roster", b"");
        relay.roster.lock().unwrap().clear();
        unauthorized(
            ureq::get(&format!("http://{address}/v1/roster"))
                .set(AUTH_HEADER, &pending.header())
                .call(),
        );
    }
    #[test]
    fn invalid_pairing_keys_do_not_mutate_membership() {
        let (_dir, _keys, _key, relay, _server, _address) = setup();
        let (code, _) = relay.start_pairing();
        let keys = DeviceKeys::generate("phone".into());
        let mut public = keys.public("Phone");
        public.exchange = [0; 32];
        let request = crate::pairing::request(&code, &public).unwrap();
        assert!(relay.pair(&request).is_err());
        assert_eq!(relay.roster().len(), 1);
        assert!(relay.pairing_active());
    }

    #[test]
    fn altered_uploads_and_wrong_device_or_response_keys_fail_without_writes() {
        let (_dir, keys, _key, relay, _server, address) = setup();
        let path = "/v1/batches/host/1";
        let p = proof(&address, &keys, "PUT", path, b"original");
        unauthorized(
            ureq::put(&format!("http://{address}{path}"))
                .set(AUTH_HEADER, &p.header())
                .send_bytes(b"altered"),
        );
        assert!(relay.store.devices().unwrap().is_empty());
        let unknown = DeviceKeys::generate("unknown".into());
        let p = proof(&address, &unknown, "GET", "/v1/roster", b"");
        unauthorized(
            ureq::get(&format!("http://{address}/v1/roster"))
                .set(AUTH_HEADER, &p.header())
                .call(),
        );
        let http = HttpStore::new(&address, "space", &keys, &SpaceKey::generate()).unwrap();
        assert!(http
            .roster()
            .unwrap_err()
            .to_string()
            .contains("unverified peer response"));
    }
}
