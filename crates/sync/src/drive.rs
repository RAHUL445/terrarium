//! Google Drive as a batch store, in the app's hidden `appDataFolder`.
//!
//! Only the `drive.appdata` scope is used: the app can see nothing else in
//! the user's Drive, and the folder is invisible in the Drive UI. Google
//! stores only ciphertext. Files: `b.<device>.<seq>` per batch,
//! `s.<device>` per snapshot.

use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use terrarium_core::DeviceId;

use crate::error::{Result, SyncError};
use crate::keys::random;
use crate::transport::BatchStore;

pub const SCOPE: &str = "https://www.googleapis.com/auth/drive.appdata";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const MAX_FILE: u64 = 128 * 1024 * 1024;

/// What a device needs to use the store. The refresh token only grants
/// access to this app's hidden folder.
#[derive(Clone, Serialize, Deserialize)]
pub struct DriveCredentials {
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

impl std::fmt::Debug for DriveCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriveCredentials")
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

/// Google's endpoints; tests point these at a fake server.
#[derive(Clone, Debug)]
pub struct DriveEndpoints {
    pub token: String,
    pub api: String,
    pub upload: String,
}

impl Default for DriveEndpoints {
    fn default() -> Self {
        DriveEndpoints {
            token: "https://oauth2.googleapis.com/token".into(),
            api: "https://www.googleapis.com/drive/v3".into(),
            upload: "https://www.googleapis.com/upload/drive/v3".into(),
        }
    }
}

pub struct DriveStore {
    creds: DriveCredentials,
    endpoints: DriveEndpoints,
    agent: ureq::Agent,
    token: Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default = "default_expiry")]
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
}

fn default_expiry() -> u64 {
    3600
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
    name: String,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .build()
}

fn drive_error(e: ureq::Error) -> SyncError {
    match e {
        ureq::Error::Status(code, _) => {
            SyncError::Transport(format!("Google Drive answered HTTP {code}"))
        }
        ureq::Error::Transport(_) => SyncError::Transport("Google Drive not reachable".into()),
    }
}

/// Percent-encodes a query value.
fn enc(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn batch_name(device: &DeviceId, seq: u64) -> String {
    format!("b.{}.{seq:020}", device.0)
}

fn snapshot_name(device: &DeviceId) -> String {
    format!("s.{}", device.0)
}

fn check_device(device: &DeviceId) -> Result<()> {
    if !device.0.is_empty()
        && device
            .0
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Ok(())
    } else {
        Err(SyncError::Transport(format!(
            "device id `{device}` is not a safe name"
        )))
    }
}

impl DriveStore {
    pub fn new(creds: DriveCredentials) -> DriveStore {
        Self::with_endpoints(creds, DriveEndpoints::default())
    }

    pub fn with_endpoints(creds: DriveCredentials, endpoints: DriveEndpoints) -> DriveStore {
        DriveStore {
            creds,
            endpoints,
            agent: agent(),
            token: Mutex::new(None),
        }
    }

    fn access_token(&self) -> Result<String> {
        let mut slot = self.token.lock().unwrap();
        if let Some((token, expires)) = slot.as_ref() {
            if Instant::now() + Duration::from_secs(60) < *expires {
                return Ok(token.clone());
            }
        }
        let response: TokenResponse = self
            .agent
            .post(&self.endpoints.token)
            .send_form(&[
                ("grant_type", "refresh_token"),
                ("client_id", &self.creds.client_id),
                ("client_secret", &self.creds.client_secret),
                ("refresh_token", &self.creds.refresh_token),
            ])
            .map_err(drive_error)?
            .into_json()
            .map_err(|e| SyncError::Transport(format!("token response: {e}")))?;
        let expires = Instant::now() + Duration::from_secs(response.expires_in);
        *slot = Some((response.access_token.clone(), expires));
        Ok(response.access_token)
    }

    fn bearer(&self) -> Result<String> {
        Ok(format!("Bearer {}", self.access_token()?))
    }

    /// Files whose name matches `query` (a Drive `q` expression).
    fn search(&self, query: &str) -> Result<Vec<DriveFile>> {
        let mut out = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = format!(
                "{}/files?spaces=appDataFolder&pageSize=1000&fields=nextPageToken%2Cfiles(id%2Cname)&q={}",
                self.endpoints.api,
                enc(query)
            );
            if let Some(token) = &page {
                url.push_str(&format!("&pageToken={}", enc(token)));
            }
            let list: FileList = self
                .agent
                .get(&url)
                .set("Authorization", &self.bearer()?)
                .call()
                .map_err(drive_error)?
                .into_json()
                .map_err(|e| SyncError::Transport(format!("file list: {e}")))?;
            out.extend(list.files);
            match list.next_page_token {
                Some(next) => page = Some(next),
                None => return Ok(out),
            }
        }
    }

    fn find(&self, name: &str) -> Result<Option<String>> {
        let files = self.search(&format!("name = '{name}'"))?;
        Ok(files.into_iter().find(|f| f.name == name).map(|f| f.id))
    }

    fn download(&self, id: &str) -> Result<Vec<u8>> {
        let response = self
            .agent
            .get(&format!("{}/files/{id}?alt=media", self.endpoints.api))
            .set("Authorization", &self.bearer()?)
            .call()
            .map_err(drive_error)?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_FILE + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(SyncError::Transport("Drive file too large".into()));
        }
        Ok(bytes)
    }

    fn create(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let boundary = format!("pp{}", B64URL.encode(random::<12>()));
        let metadata =
            serde_json::json!({ "name": name, "parents": ["appDataFolder"] }).to_string();
        let mut body = Vec::with_capacity(bytes.len() + 512);
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        self.agent
            .post(&format!(
                "{}/files?uploadType=multipart&fields=id",
                self.endpoints.upload
            ))
            .set("Authorization", &self.bearer()?)
            .set(
                "Content-Type",
                &format!("multipart/related; boundary={boundary}"),
            )
            .send_bytes(&body)
            .map_err(drive_error)?;
        Ok(())
    }

    fn replace(&self, id: &str, bytes: &[u8]) -> Result<()> {
        self.agent
            .request(
                "PATCH",
                &format!("{}/files/{id}?uploadType=media", self.endpoints.upload),
            )
            .set("Authorization", &self.bearer()?)
            .set("Content-Type", "application/octet-stream")
            .send_bytes(bytes)
            .map_err(drive_error)?;
        Ok(())
    }
}

impl BatchStore for DriveStore {
    fn devices(&self) -> Result<Vec<DeviceId>> {
        let mut devices: Vec<DeviceId> = self
            .search("name contains 'b.'")?
            .into_iter()
            .filter_map(|f| {
                let rest = f.name.strip_prefix("b.")?;
                let (device, _) = rest.rsplit_once('.')?;
                Some(DeviceId::from(device))
            })
            .filter(|d| check_device(d).is_ok())
            .collect();
        devices.sort();
        devices.dedup();
        Ok(devices)
    }

    fn list(&self, device: &DeviceId, after: u64) -> Result<Vec<u64>> {
        check_device(device)?;
        let prefix = format!("b.{}.", device.0);
        let mut seqs: Vec<u64> = self
            .search(&format!("name contains '{prefix}'"))?
            .into_iter()
            .filter_map(|f| f.name.strip_prefix(&prefix)?.parse().ok())
            .filter(|seq| *seq > after)
            .collect();
        seqs.sort_unstable();
        seqs.dedup();
        Ok(seqs)
    }

    fn get(&self, device: &DeviceId, seq: u64) -> Result<Vec<u8>> {
        check_device(device)?;
        let id = self
            .find(&batch_name(device, seq))?
            .ok_or_else(|| SyncError::Transport(format!("no batch {device}/{seq} in Drive")))?;
        self.download(&id)
    }

    fn put(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()> {
        check_device(device)?;
        let name = batch_name(device, seq);
        match self.find(&name)? {
            Some(id) => self.replace(&id, bytes),
            None => self.create(&name, bytes),
        }
    }

    fn snapshot(&self, device: &DeviceId) -> Result<Option<Vec<u8>>> {
        check_device(device)?;
        match self.find(&snapshot_name(device))? {
            Some(id) => self.download(&id).map(Some),
            None => Ok(None),
        }
    }

    fn put_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()> {
        check_device(device)?;
        let name = snapshot_name(device);
        match self.find(&name)? {
            Some(id) => self.replace(&id, bytes),
            None => self.create(&name, bytes),
        }
    }
}

/// Browser sign-in for installed apps (loopback redirect with PKCE).
/// `open` shows the consent URL to the user, e.g. in the system browser.
/// Blocks until Google redirects back, or for at most 5 minutes.
pub fn authorize(
    client_id: &str,
    client_secret: &str,
    endpoints: &DriveEndpoints,
    auth_url: Option<&str>,
    open: impl FnOnce(&str) -> std::result::Result<(), String>,
) -> Result<DriveCredentials> {
    let verifier = B64URL.encode(random::<32>());
    let challenge = B64URL.encode(Sha256::digest(verifier.as_bytes()));
    let state = B64URL.encode(random::<16>());
    let expected_state = state.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let server = crate::peer::PeerServer::callback(move |method, url| {
        use crate::peer::callback_reply;
        if method != hyper::Method::GET || url.split('?').next() != Some("/") {
            return callback_reply(404, "Not found.");
        }
        let query = url.split_once('?').map(|(_, q)| q).unwrap_or_default();
        let param = |key: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
                .map(percent_decode)
        };
        if param("state").as_deref() != Some(expected_state.as_str()) {
            return callback_reply(400, "Unexpected request.");
        }
        if param("error").is_some() {
            let _ = sender.try_send(Err(()));
            return callback_reply(200, "Sign-in was cancelled. You can close this tab.");
        }
        let Some(code) = param("code") else {
            return callback_reply(400, "Missing code.");
        };
        let _ = sender.try_send(Ok(code));
        callback_reply(
            200,
            "Sign-in received. Return to Terrarium to confirm the connection.",
        )
    })?;
    let port = server.addrs()[0].port();
    let redirect = format!("http://127.0.0.1:{port}");
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}&access_type=offline&prompt=consent",
        auth_url.unwrap_or(AUTH_URL),
        enc(client_id),
        enc(&redirect),
        enc(SCOPE),
    );
    open(&url).map_err(SyncError::Transport)?;

    let code = receiver
        .recv_timeout(Duration::from_secs(300))
        .map_err(|_| SyncError::Transport("Google sign-in timed out".into()))?
        .map_err(|_| SyncError::Transport("Google sign-in was cancelled".into()))?;

    let response: TokenResponse = agent()
        .post(&endpoints.token)
        .send_form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", &redirect),
            ("code_verifier", &verifier),
        ])
        .map_err(drive_error)?
        .into_json()
        .map_err(|e| SyncError::Transport(format!("token response: {e}")))?;
    let refresh_token = response
        .refresh_token
        .ok_or_else(|| SyncError::Transport("Google did not return a refresh token".into()))?;
    Ok(DriveCredentials {
        client_id: client_id.into(),
        client_secret: client_secret.into(),
        refresh_token,
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_round_trip() {
        let original = "name contains 'b.dev-1.' & x=1/2";
        assert_eq!(percent_decode(&enc(original)), original);
        assert_eq!(percent_decode("a+b%2Fc%zz"), "a b/c%zz");
    }
}
