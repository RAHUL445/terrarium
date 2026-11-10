//! Space membership and sync for this device.
//!
//! A desktop can host the space: it keeps the batch folder, serves it to
//! paired devices over HTTP on its Tailscale address (and loopback), and runs
//! pairing. A phone joins a host with a pairing code and then syncs through
//! it. Every device keeps a full copy and works offline; sync runs in the
//! background every few seconds and on demand.
//!
//! Secrets (this device's keys and the space key) go to [`Secrets`]: the
//! macOS Keychain, the Android Keystore, or owner-only files. `space.json`
//! in `<space>/sync/` holds the space id, role, host address and roster.

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, Manager, State};
use terrarium_core::DeviceId;
use terrarium_sync::{
    authorize_drive, pair_with, pairing, Backup, BatchStore, DeviceKeys, DriveCredentials,
    DriveEndpoints, DriveStore, Envelope, FolderStore, HttpStore, Kind, MultiStore, PeerServer,
    Relay, Roster, SpaceKey, SpaceSync, SyncReport, BACKUP_EXT,
};

use crate::plugins::Plugins;
use crate::secrets::{write_private, Secrets};

const IDENTITY: &str = "identity";
const DRIVE: &str = "drive";
const SPACE_KEY: &str = "space-key";

pub const PORT: u16 = 7810;
const INTERVAL: Duration = Duration::from_secs(10);
/// The host compacts once its folder holds this many batches.
const COMPACT_AT: usize = 200;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    Host,
    Peer,
}

#[derive(Clone, Serialize, Deserialize)]
struct SpaceFile {
    space: String,
    role: Role,
    /// For a peer: the host's `address:port`.
    #[serde(default)]
    host: Option<String>,
    /// Only in files written before keys moved to [`Secrets`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    roster: Roster,
}

#[derive(Clone, Serialize)]
pub struct LastSync {
    at_ms: u64,
    ok: bool,
    error: Option<String>,
    report: Option<SyncReport>,
    /// Stores that could not be reached, e.g. the host while it sleeps.
    stores: BTreeMap<String, String>,
}

struct Membership {
    file: SpaceFile,
    key: SpaceKey,
    engine: SpaceSync,
}

struct Inner {
    dir: PathBuf,
    secrets: Secrets,
    drive: Option<DriveCredentials>,
    space: String,
    keys: DeviceKeys,
    name: String,
    member: Option<Membership>,
    last: Option<LastSync>,
}

pub struct Sync {
    inner: Mutex<Inner>,
    relay: Mutex<Option<Arc<Relay>>>,
    server: Mutex<Option<(Vec<SocketAddr>, PeerServer)>>,
}

#[derive(Serialize)]
pub struct DeviceInfo {
    device: String,
    name: String,
    this_device: bool,
}

#[derive(Serialize)]
pub struct Status {
    /// Where this device keeps its keys.
    key_storage: &'static str,
    drive: bool,
    device: String,
    name: String,
    /// `null` until this device hosts or joins a space.
    role: Option<Role>,
    can_host: bool,
    host: Option<String>,
    devices: Vec<DeviceInfo>,
    /// Where peers can reach this host; empty without Tailscale.
    tailnet_addresses: Vec<String>,
    pairing: bool,
    last: Option<LastSync>,
}

#[derive(Serialize)]
pub struct PairingCode {
    pub code: String,
    pub expires_in_s: u64,
    addresses: Vec<String>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// IPv4 addresses in Tailscale's 100.64.0.0/10 range on this machine.
fn tailnet_ips() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    let mut out: Vec<Ipv4Addr> = interfaces
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) if ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]) => {
                Some(ip)
            }
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn device_name() -> String {
    match std::env::consts::OS {
        "macos" => "Mac".into(),
        "android" => "Android phone".into(),
        "ios" => "iPhone".into(),
        other => other.to_string(),
    }
}

impl Sync {
    pub fn open(space_dir: &Path, space: &str, device: DeviceId) -> Result<Sync, String> {
        let dir = space_dir.join("sync");
        fs::create_dir_all(&dir).map_err(err)?;
        // Existing membership must remain recoverable if credential storage
        // fails; never silently generate a replacement device identity for it.
        let stored_space: Option<SpaceFile> =
            crate::secrets::read_private(&dir.join("space.json"))?
                .map(|bytes| serde_json::from_slice(&bytes))
                .transpose()
                .map_err(|_| "Stored space membership is invalid; original data was preserved")?;
        let secrets = Secrets::open(&dir, space)?;
        // Keys written by earlier versions as plain files move in once.
        let legacy_identity = dir.join("identity.json");
        secrets.adopt(IDENTITY, &legacy_identity, |b| Some(b.to_vec()))?;
        let keys = match secrets.load(IDENTITY)? {
            Some(bytes) => serde_json::from_slice::<DeviceKeys>(&bytes)
                .map_err(|_| "Stored device identity is invalid; original data was preserved")?,
            None if stored_space.is_some() => return Err("Device identity is missing for an existing space; restore credentials before continuing".into()),
            None => {
                let keys = DeviceKeys::generate(device.clone());
                secrets.save(IDENTITY, &serde_json::to_vec(&keys).map_err(err)?)?;
                keys
            }
        };
        if keys.device != device {
            return Err("the stored identity belongs to another device id".into());
        }
        match fs::remove_file(&legacy_identity) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err("Identity was verified but its legacy file could not be removed".into())
            }
        }
        let drive = match secrets.load(DRIVE)? {
            Some(bytes) => serde_json::from_slice(&bytes).ok(),
            None => None,
        };
        let mut inner = Inner {
            dir,
            secrets,
            drive,
            space: space.to_string(),
            keys,
            name: device_name(),
            member: None,
            last: None,
        };
        if let Some(mut file) = stored_space {
            if let Some(legacy) = file.key.take() {
                let key: [u8; 32] = B64.decode(legacy).map_err(err)?.try_into().map_err(|_| {
                    "Legacy space key has the wrong length; original data was preserved"
                })?;
                inner.secrets.save_verified(SPACE_KEY, &key)?;
                save_space(&inner.dir, &file)?;
            }
            inner.member = Some(inner.membership(file)?);
        }
        Ok(Sync {
            inner: Mutex::new(inner),
            relay: Mutex::new(None),
            server: Mutex::new(None),
        })
    }

    /// Starts hosting (if this device hosts) and the background sync loop.
    pub fn start(app: AppHandle) {
        std::thread::spawn(move || loop {
            let state = app.state::<Sync>();
            state.ensure_hosting();
            state.refresh_host_roster();
            let plugins = app.state::<Plugins>();
            if let Some(last) = state.run(&plugins) {
                if last.report.as_ref().is_some_and(|r| r.applied > 0) {
                    let _ = app.emit("data-changed", ());
                }
                let _ = app.emit("sync-status", last);
            }
            std::thread::sleep(INTERVAL);
        });
    }

    /// One sync pass; `None` when this device is not in a space yet.
    fn run(&self, plugins: &Plugins) -> Option<LastSync> {
        let mut inner = self.inner.lock().ok()?;
        let Inner {
            member,
            dir,
            secrets,
            drive,
            keys,
            ..
        } = &mut *inner;
        let member = member.as_mut()?;
        let mut store_errors = BTreeMap::new();
        let outcome = (|| -> Result<SyncReport, String> {
            let mut stores: Vec<(String, Box<dyn BatchStore + Send + std::marker::Sync>)> =
                Vec::new();
            match member.file.role {
                Role::Host => stores.push((
                    "this Mac".into(),
                    Box::new(FolderStore::new(dir.join("relay"))),
                )),
                Role::Peer => {
                    let host = member.file.host.clone().ok_or("no host address saved")?;
                    let http = HttpStore::new(&host, &member.file.space, keys, &member.key)
                        .map_err(err)?;
                    // Devices paired with the host after this one joined.
                    if let Ok(roster) = http.roster() {
                        if roster != member.file.roster
                            && roster.contains_key(member.engine.device())
                        {
                            member.file.roster = roster.clone();
                            member.engine.set_roster(roster);
                            save_space(dir, &member.file)?;
                        }
                    }
                    // Drive credentials the host shares, sealed with the space key.
                    if drive.is_none() {
                        if let Ok(Some(sealed)) = http.config(DRIVE) {
                            let opened = Envelope::from_bytes(&sealed).and_then(|e| {
                                e.open_bytes(&member.file.space, &member.key, &member.file.roster)
                            });
                            if let Ok(bytes) = opened {
                                if let Ok(creds) =
                                    serde_json::from_slice::<DriveCredentials>(&bytes)
                                {
                                    secrets.save(DRIVE, &bytes)?;
                                    *drive = Some(creds);
                                }
                            }
                        }
                    }
                    stores.push(("host".into(), Box::new(http)));
                }
            }
            if let Some(creds) = drive.clone() {
                stores.push(("Google Drive".into(), Box::new(DriveStore::new(creds))));
            }
            let transport = MultiStore::new(stores);
            let result = plugins
                .with_host(|host| member.engine.sync(host, &transport).map_err(err))
                .unwrap_or_else(|| Err("plugin host unavailable".into()));
            store_errors = transport.errors();
            let report = result?;
            if member.file.role == Role::Host {
                let folder = FolderStore::new(dir.join("relay"));
                let stored: usize = folder
                    .devices()
                    .map_err(err)?
                    .iter()
                    .map(|d| folder.list(d, 0).map(|s| s.len()).unwrap_or(0))
                    .sum();
                if stored >= COMPACT_AT {
                    let removed = plugins
                        .with_host(|host| member.engine.compact(host, &folder).map_err(err))
                        .ok_or("plugin host unavailable")??;
                    eprintln!("compacted {removed} of {stored} batches");
                }
            }
            Ok(report)
        })();
        let last = match outcome {
            Ok(report) => LastSync {
                at_ms: now_ms(),
                ok: report.rejected.is_empty(),
                error: None,
                report: Some(report),
                stores: store_errors,
            },
            Err(e) => LastSync {
                at_ms: now_ms(),
                ok: false,
                error: Some(e),
                report: None,
                stores: store_errors,
            },
        };
        inner.last = Some(last.clone());
        Some(last)
    }

    /// Keeps the peer server listening on loopback plus whatever tailnet
    /// address this machine has now (Tailscale may connect later).
    fn ensure_hosting(&self) {
        if !cfg!(desktop) {
            return;
        }
        let relay = {
            let Ok(inner) = self.inner.lock() else { return };
            let Some(member) = inner.member.as_ref().filter(|m| m.file.role == Role::Host) else {
                return;
            };
            let mut slot = self.relay.lock().unwrap();
            if slot.is_none() {
                let dir = inner.dir.clone();
                let roster_dir = dir.clone();
                *slot = Some(Arc::new(Relay::new(
                    &member.file.space,
                    member.key.clone(),
                    FolderStore::new(dir.join("relay")),
                    member.file.roster.clone(),
                    Box::new(move |roster: &Roster| {
                        let bytes = crate::secrets::read_private(&roster_dir.join("space.json"))
                            .map_err(terrarium_sync::SyncError::Transport)?
                            .ok_or_else(|| {
                                terrarium_sync::SyncError::Transport(
                                    "space membership missing".into(),
                                )
                            })?;
                        let mut file: SpaceFile = serde_json::from_slice(&bytes)?;
                        file.roster = roster.clone();
                        save_space(&roster_dir, &file).map_err(terrarium_sync::SyncError::Transport)
                    }),
                )));
            }
            let relay = slot.clone().unwrap();
            relay.set_config(DRIVE, inner.sealed_drive(member));
            relay
        };
        let mut wanted: Vec<SocketAddr> = vec![SocketAddr::from(([127, 0, 0, 1], PORT))];
        wanted.extend(
            tailnet_ips()
                .into_iter()
                .map(|ip| SocketAddr::from((ip, PORT))),
        );
        let mut server = self.server.lock().unwrap();
        if server.as_ref().is_some_and(|(addrs, _)| *addrs == wanted) {
            return;
        }
        *server = None; // stop the old listeners before binding again
        match PeerServer::start(&wanted, relay) {
            Ok(s) => {
                eprintln!("sync host listening on {wanted:?}");
                *server = Some((wanted, s));
            }
            Err(e) => eprintln!("sync host could not listen: {e}"),
        }
    }

    /// The roster as the relay knows it (pairing updates it live).
    fn refresh_host_roster(&self) {
        let relay = self.relay.lock().unwrap().clone();
        if let (Some(relay), Ok(mut inner)) = (relay, self.inner.lock()) {
            if let Some(member) = inner.member.as_mut().filter(|m| m.file.role == Role::Host) {
                let roster = relay.roster();
                if roster != member.file.roster {
                    member.file.roster = roster.clone();
                    member.engine.set_roster(roster);
                }
            }
        }
    }
}

fn save_space(dir: &Path, file: &SpaceFile) -> Result<(), String> {
    write_private(
        &dir.join("space.json"),
        &serde_json::to_vec_pretty(file).map_err(err)?,
    )
    .map_err(err)
}

impl Inner {
    /// Drive credentials sealed for peers (only paired devices can open).
    fn sealed_drive(&self, member: &Membership) -> Option<Vec<u8>> {
        let creds = self.drive.as_ref()?;
        let json = serde_json::to_vec(creds).ok()?;
        Envelope::seal_bytes(
            Kind::Config,
            &member.file.space,
            0,
            &json,
            &member.key,
            &self.keys,
        )
        .ok()?
        .to_bytes()
        .ok()
    }

    fn membership(&self, file: SpaceFile) -> Result<Membership, String> {
        let bytes: [u8; 32] = self
            .secrets
            .load(SPACE_KEY)?
            .ok_or("this device is in a space but its space key is missing")?
            .try_into()
            .map_err(|_| "space key has the wrong length".to_string())?;
        let key = SpaceKey::from_bytes(bytes);
        let engine = SpaceSync::open(
            &file.space,
            self.keys.clone(),
            key.clone(),
            file.roster.clone(),
            &self.dir,
        )
        .map_err(err)?;
        Ok(Membership { file, key, engine })
    }

    /// Makes this device the host of a space with `key`. `others` are
    /// devices to keep accepting (from a backup's roster).
    fn host_new_space(&mut self, key: SpaceKey, others: Roster) -> Result<(), String> {
        let mut roster = others;
        roster.insert(self.keys.device.clone(), self.keys.public(&self.name));
        let file = SpaceFile {
            space: self.space.clone(),
            role: Role::Host,
            host: None,
            key: None,
            roster,
        };
        self.join(file, &key)
    }

    fn join(&mut self, file: SpaceFile, key: &SpaceKey) -> Result<(), String> {
        self.secrets.save(SPACE_KEY, &key.to_bytes())?;
        save_space(&self.dir, &file)?;
        self.member = Some(self.membership(file)?);
        Ok(())
    }
}

#[tauri::command]
pub fn sync_status(sync: State<'_, Sync>) -> Result<Status, String> {
    sync.refresh_host_roster();
    let pairing = sync
        .relay
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|r| r.pairing_active());
    let inner = sync.inner.lock().map_err(err)?;
    let roster = inner
        .member
        .as_ref()
        .map(|m| m.file.roster.clone())
        .unwrap_or_default();
    let mut devices: Vec<DeviceInfo> = roster
        .values()
        .map(|d| DeviceInfo {
            device: d.device.0.clone(),
            name: d.name.clone(),
            this_device: d.device == inner.keys.device,
        })
        .collect();
    if devices.is_empty() {
        devices.push(DeviceInfo {
            device: inner.keys.device.0.clone(),
            name: inner.name.clone(),
            this_device: true,
        });
    }
    Ok(Status {
        key_storage: inner.secrets.describe(),
        drive: inner.drive.is_some(),
        device: inner.keys.device.0.clone(),
        name: inner.name.clone(),
        role: inner.member.as_ref().map(|m| m.file.role),
        can_host: cfg!(desktop),
        host: inner.member.as_ref().and_then(|m| m.file.host.clone()),
        devices,
        tailnet_addresses: tailnet_ips()
            .iter()
            .map(|ip| format!("{ip}:{PORT}"))
            .collect(),
        pairing,
        last: inner.last.clone(),
    })
}

/// Blocks a device at this host. Existing copies, keys and Drive access remain.
#[tauri::command]
pub fn remove_sync_device(sync: State<'_, Sync>, device: String) -> Result<(), String> {
    sync.ensure_hosting();
    let mut inner = sync.inner.lock().map_err(err)?;
    let host = inner.keys.device.clone();
    let member = inner
        .member
        .as_mut()
        .filter(|m| m.file.role == Role::Host)
        .ok_or("Only the space host can remove a device")?;
    let relay = sync
        .relay
        .lock()
        .map_err(err)?
        .clone()
        .ok_or("Sync host unavailable")?;
    relay.remove_device(&DeviceId(device), &host).map_err(err)?;
    member.file.roster = relay.roster();
    member.engine.set_roster(member.file.roster.clone());
    Ok(())
}

/// Makes this device the space host if it is not in a space yet, then
/// shows a one-time pairing code.
#[tauri::command]
pub fn start_pairing(sync: State<'_, Sync>) -> Result<PairingCode, String> {
    sync.begin_pairing()
}

impl Sync {
    /// What seals host secrets (the AI key): the space key once this device
    /// is in a space, so every paired device can open them; before that a
    /// key kept only on this device.
    pub fn secrets_root(&self) -> Result<(&'static str, [u8; 32]), String> {
        const LOCAL: &str = "secrets-key";
        let inner = self.inner.lock().map_err(err)?;
        if let Some(member) = &inner.member {
            return Ok(("space", member.key.to_bytes()));
        }
        if let Some(bytes) = inner.secrets.load(LOCAL)? {
            if let Ok(root) = <[u8; 32]>::try_from(bytes.as_slice()) {
                return Ok(("device", root));
            }
        }
        let mut root = [0u8; 32];
        getrandom::getrandom(&mut root).map_err(err)?;
        inner.secrets.save(LOCAL, &root)?;
        Ok(("device", root))
    }
}

impl Sync {
    pub fn begin_pairing(&self) -> Result<PairingCode, String> {
        let sync = self;
        if !cfg!(desktop) {
            return Err("Pair new devices from your computer.".into());
        }
        {
            let mut inner = sync.inner.lock().map_err(err)?;
            match inner.member.as_ref().map(|m| m.file.role) {
                Some(Role::Peer) => return Err("This device already joined another host.".into()),
                Some(Role::Host) => {}
                None => inner.host_new_space(SpaceKey::generate(), Roster::new())?,
            }
        }
        sync.ensure_hosting();
        let relay = sync
            .relay
            .lock()
            .unwrap()
            .clone()
            .ok_or("sync host is not running")?;
        let (code, ttl) = relay.start_pairing();
        Ok(PairingCode {
            code: pairing::display_code(&code),
            expires_in_s: ttl.as_secs(),
            addresses: tailnet_ips()
                .iter()
                .map(|ip| format!("{ip}:{PORT}"))
                .collect(),
        })
    }
}

#[tauri::command]
pub fn cancel_pairing(sync: State<'_, Sync>) {
    if let Some(relay) = sync.relay.lock().unwrap().as_ref() {
        relay.cancel_pairing();
    }
}

/// Joins the space hosted at `host` (`address` or `address:port`).
#[tauri::command]
pub async fn join_space(app: AppHandle, host: String, code: String) -> Result<(), String> {
    let host = host.trim().to_string();
    if host.is_empty() {
        return Err("Enter the host address shown on your computer.".into());
    }
    let host = if host.contains(':') {
        host
    } else {
        format!("{host}:{PORT}")
    };
    tauri::async_runtime::spawn_blocking(move || {
        let sync = app.state::<Sync>();
        let (keys, name, space) = {
            let inner = sync.inner.lock().map_err(err)?;
            if inner.member.is_some() {
                return Err("This device is already in a space.".to_string());
            }
            (inner.keys.clone(), inner.name.clone(), inner.space.clone())
        };
        let joined = pair_with(&host, &code, &keys, &name).map_err(err)?;
        if joined.space != space {
            return Err(format!(
                "The host's space is `{}`, this device uses `{space}`.",
                joined.space
            ));
        }
        let file = SpaceFile {
            space: joined.space,
            role: Role::Peer,
            host: Some(host),
            key: None,
            roster: joined.roster,
        };
        sync.inner.lock().map_err(err)?.join(file, &joined.key)?;
        let plugins = app.state::<Plugins>();
        if let Some(last) = sync.run(&plugins) {
            let _ = app.emit("data-changed", ());
            let _ = app.emit("sync-status", last);
        }
        Ok(())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn sync_now(app: AppHandle) -> Result<Option<LastSync>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sync = app.state::<Sync>();
        sync.ensure_hosting();
        sync.refresh_host_roster();
        let last = sync.run(&app.state::<Plugins>());
        if let Some(last) = &last {
            if last.report.as_ref().is_some_and(|r| r.applied > 0) {
                let _ = app.emit("data-changed", ());
            }
        }
        last
    })
    .await
    .map_err(err)
}

/// The 24-word phrase that restores the space key; shown on request only.
#[tauri::command]
pub fn recovery_phrase(sync: State<'_, Sync>) -> Result<String, String> {
    let inner = sync.inner.lock().map_err(err)?;
    let member = inner
        .member
        .as_ref()
        .ok_or("This device is not in a space yet.")?;
    Ok(member.key.recovery_phrase())
}

fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open the browser: {e}"))
}

/// Signs in to Google in the browser and stores the Drive credentials.
/// Only the host does this; peers receive the credentials sealed with the
/// space key the next time they sync.
#[tauri::command]
pub async fn connect_drive(
    app: AppHandle,
    client_id: String,
    client_secret: String,
) -> Result<(), String> {
    if !cfg!(desktop) {
        return Err("Connect Google Drive on the computer that hosts the space.".into());
    }
    let (client_id, client_secret) = (
        client_id.trim().to_string(),
        client_secret.trim().to_string(),
    );
    if client_id.is_empty() || client_secret.is_empty() {
        return Err("Enter the OAuth client ID and secret.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let sync = app.state::<Sync>();
        {
            let inner = sync.inner.lock().map_err(err)?;
            if inner.member.as_ref().map(|m| m.file.role) != Some(Role::Host) {
                return Err("Pair a device first; Drive is connected on the host.".to_string());
            }
        }
        let creds = authorize_drive(
            &client_id,
            &client_secret,
            &DriveEndpoints::default(),
            None,
            open_browser,
        )
        .map_err(err)?;
        // Prove the credentials work before keeping them.
        DriveStore::new(creds.clone()).devices().map_err(err)?;
        let mut inner = sync.inner.lock().map_err(err)?;
        inner
            .secrets
            .save(DRIVE, &serde_json::to_vec(&creds).map_err(err)?)?;
        inner.drive = Some(creds);
        let sealed = inner.member.as_ref().and_then(|m| inner.sealed_drive(m));
        drop(inner);
        if let Some(relay) = sync.relay.lock().unwrap().as_ref() {
            relay.set_config(DRIVE, sealed);
        }
        Ok(())
    })
    .await
    .map_err(err)?
}

/// Stops using Drive on this device (peers keep their copy until they
/// are removed; revoke access in your Google account to cut everyone off).
#[tauri::command]
pub fn disconnect_drive(sync: State<'_, Sync>) -> Result<(), String> {
    let mut inner = sync.inner.lock().map_err(err)?;
    inner.secrets.remove(DRIVE)?;
    inner.drive = None;
    drop(inner);
    if let Some(relay) = sync.relay.lock().unwrap().as_ref() {
        relay.set_config(DRIVE, None);
    }
    Ok(())
}

// ---- Encrypted backups -------------------------------------------------

#[derive(Serialize)]
pub struct BackupEntry {
    path: String,
    name: String,
    created_ms: u64,
    size: u64,
}

#[derive(Serialize)]
pub struct RestoreSummary {
    applied: usize,
    duplicates: usize,
    /// Plugins in the backup that are not installed here.
    missing: Vec<String>,
    /// This device took over the backup's space as its host.
    adopted_space: bool,
}

/// Desktop: `~/Documents/Terrarium Backups`; phone: app data.
fn backups_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = if cfg!(desktop) {
        app.path()
            .document_dir()
            .map_err(err)?
            .join("Terrarium Backups")
    } else {
        app.path().app_data_dir().map_err(err)?.join("backups")
    };
    fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir)
}

fn entry(path: &Path) -> Option<BackupEntry> {
    let bytes = fs::read(path).ok()?;
    let info = Backup::info(&bytes).ok()?;
    Some(BackupEntry {
        path: path.display().to_string(),
        name: path.file_name()?.to_string_lossy().into_owned(),
        created_ms: info.created_ms,
        size: bytes.len() as u64,
    })
}

/// Writes an encrypted backup of every plugin's data and the plugin set.
/// Readable with the space's recovery phrase only.
#[tauri::command]
pub async fn create_backup(app: AppHandle) -> Result<BackupEntry, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let sync = app.state::<Sync>();
        let (key, roster, device) = {
            let mut inner = sync.inner.lock().map_err(err)?;
            if inner.member.is_none() {
                if !cfg!(desktop) {
                    return Err("Join your computer first; backups are encrypted with the space's recovery phrase.".into());
                }
                inner.host_new_space(SpaceKey::generate(), Roster::new())?;
            }
            let member = inner.member.as_ref().ok_or("not in a space")?;
            (member.key.clone(), member.file.roster.clone(), inner.keys.device.clone())
        };
        let space = sync.inner.lock().map_err(err)?.space.clone();
        let plugins = app.state::<Plugins>();
        let now = now_ms();
        let sealed = plugins
            .with_host(|host| {
                Backup::collect(&space, &device, now, roster, host).and_then(|b| b.seal(&key))
            })
            .ok_or("plugin host unavailable")?
            .map_err(err)?;
        let stamp = time::OffsetDateTime::from_unix_timestamp((now / 1000) as i64)
            .map_err(err)?;
        let name = format!(
            "backup-{:04}{:02}{:02}-{:02}{:02}{:02}Z.{BACKUP_EXT}",
            stamp.year(),
            u8::from(stamp.month()),
            stamp.day(),
            stamp.hour(),
            stamp.minute(),
            stamp.second()
        );
        let path = backups_dir(&app)?.join(name);
        write_private(&path, &sealed).map_err(err)?;
        entry(&path).ok_or_else(|| "backup was written but cannot be read back".to_string())
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub fn list_backups(app: AppHandle) -> Result<Vec<BackupEntry>, String> {
    let dir = backups_dir(&app)?;
    let mut out: Vec<BackupEntry> = fs::read_dir(&dir)
        .map_err(err)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == BACKUP_EXT))
        .filter_map(|p| entry(&p))
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.created_ms));
    Ok(out)
}

/// Merges a backup into this device: from a file path (desktop) or an
/// uploaded file (base64). A fresh device needs the recovery phrase; a
/// fresh computer then hosts the backup's space, so devices paired before
/// keep syncing.
#[tauri::command]
pub async fn restore_backup(
    app: AppHandle,
    path: Option<String>,
    data: Option<String>,
    phrase: Option<String>,
) -> Result<RestoreSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = match (path, data) {
            (Some(path), _) => fs::read(&path).map_err(|e| format!("{path}: {e}"))?,
            (None, Some(data)) => B64.decode(data).map_err(err)?,
            _ => return Err("Choose a backup file.".into()),
        };
        let phrase = phrase
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        let sync = app.state::<Sync>();
        let (backup, adopted_space) = {
            let mut inner = sync.inner.lock().map_err(err)?;
            let from_phrase = phrase
                .as_deref()
                .map(SpaceKey::from_recovery_phrase)
                .transpose()
                .map_err(err)?;
            let key = match (&inner.member, from_phrase) {
                (Some(m), Some(k)) if k.to_bytes() != m.key.to_bytes() => {
                    return Err(
                        "That phrase belongs to a different space than this device's. \
                                Restore it on a fresh install, or leave the phrase empty."
                            .into(),
                    )
                }
                (Some(m), _) => m.key.clone(),
                (None, Some(k)) => k,
                (None, None) => {
                    return Err(
                        "Enter the recovery phrase of the space this backup came from.".into(),
                    )
                }
            };
            let backup = Backup::open(&bytes, &key).map_err(err)?;
            let adopt = inner.member.is_none() && cfg!(desktop);
            if adopt {
                inner.host_new_space(key, backup.roster.clone())?;
            }
            (backup, adopt)
        };
        let plugins = app.state::<Plugins>();
        // The plugin set first; then the plugins it brings can take their data.
        let first = plugins
            .with_host(|host| backup.restore(host))
            .ok_or("plugin host unavailable")?
            .map_err(err)?;
        plugins.reconcile(&crate::shell::AppShell(app.clone()));
        let second = plugins
            .with_host(|host| backup.restore(host))
            .ok_or("plugin host unavailable")?
            .map_err(err)?;
        let _ = app.emit("data-changed", ());
        Ok(RestoreSummary {
            applied: first.applied + second.applied,
            duplicates: second.duplicates,
            missing: second.missing,
            adopted_space,
        })
    })
    .await
    .map_err(err)?
}
