//! Client side of the peer protocol: a [`BatchStore`] backed by a host
//! device over HTTP, and pairing with it.

use std::io::Read;
use std::time::Duration;

use terrarium_core::DeviceId;

use crate::error::{Result, SyncError};
use crate::keys::{DeviceKeys, Roster};
use crate::pairing::{self, Joined, PairResponse};
use crate::transport::BatchStore;

const MAX_BATCH: u64 = 32 * 1024 * 1024;
const MAX_SNAPSHOT: u64 = MAX_BATCH * 4;
const MAX_METADATA: u64 = 1024 * 1024;
const MAX_BLOB_BODY: u64 = 26 * 1024 * 1024;
const MAX_BLOB_LIST_LIMIT: usize = 5_000;

fn read_limited(reader: impl Read, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SyncError::Transport("could not read peer response".into()))?;
    if bytes.len() as u64 > limit {
        return Err(SyncError::Transport(
            "peer response exceeds its size limit".into(),
        ));
    }
    Ok(bytes)
}

fn check_status(response: ureq::Response) -> Result<ureq::Response> {
    if !(200..300).contains(&response.status()) {
        return Err(SyncError::Transport(format!(
            "host answered HTTP {}",
            response.status()
        )));
    }
    Ok(response)
}

fn read_response(response: ureq::Response, limit: u64) -> Result<Vec<u8>> {
    let response = check_status(response)?;
    if response
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|len| len > limit)
    {
        return Err(SyncError::Transport(
            "peer response exceeds its size limit".into(),
        ));
    }
    read_limited(response.into_reader(), limit)
}

pub struct HttpStore {
    base: String,
    agent: ureq::Agent,
    keys: DeviceKeys,
    key: crate::SpaceKey,
    space: String,
}

/// `host` is `address:port`, e.g. `100.101.102.103:7810` or a full MagicDNS `.ts.net` name.
fn base_url(host: &str) -> Result<String> {
    let host = host.trim();
    if host.len() > 2048 || host.chars().any(char::is_whitespace) {
        return Err(SyncError::Transport("invalid peer address".into()));
    }
    let address = if host.contains("://") {
        host.to_string()
    } else {
        format!("http://{host}")
    };
    let url = url::Url::parse(&address)
        .map_err(|_| SyncError::Transport("invalid peer address".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || url.host().is_none()
    {
        return Err(SyncError::Transport(
            "Use a peer origin without credentials, paths, query or fragment".into(),
        ));
    }
    let private = match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            ip.is_loopback() || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
        }
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(name)) => name == "localhost" || name.ends_with(".ts.net"),
        None => false,
    };
    if url.scheme() == "http" && !private {
        return Err(SyncError::Transport("HTTP peers require loopback or a Tailscale IP/full .ts.net name; use HTTPS for other hosts".into()));
    }
    Ok(format!("{}v1", url.as_str()))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .redirects(0)
        .try_proxy_from_env(false)
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
}

fn transport_error(e: ureq::Error) -> SyncError {
    match e {
        // Do not echo untrusted bodies or URL-bearing transport errors into logs.
        ureq::Error::Status(code, _) => SyncError::Transport(format!("host answered HTTP {code}")),
        ureq::Error::Transport(_) => SyncError::Transport("host not reachable".into()),
    }
}

impl HttpStore {
    /// Uses existing pairing keys; never falls back to unsigned requests/responses.
    pub fn new(
        host: &str,
        space: &str,
        keys: &DeviceKeys,
        key: &crate::SpaceKey,
    ) -> Result<HttpStore> {
        crate::transport::check_device(&keys.device)?;
        Ok(HttpStore {
            base: base_url(host)?,
            agent: agent(),
            keys: keys.clone(),
            key: key.clone(),
            space: space.into(),
        })
    }

    fn exchange(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
        limit: u64,
    ) -> Result<(u16, Vec<u8>)> {
        // Challenges are public and short-lived. Authentication proves device-key
        // possession, not mere knowledge of the shared space encryption key.
        let challenge = self
            .agent
            .get(&format!("{}/challenge", self.base))
            .call()
            .map_err(transport_error)?;
        #[derive(serde::Deserialize)]
        struct Challenge {
            nonce: String,
        }
        let challenge: Challenge = serde_json::from_slice(&read_response(challenge, 1024)?)?;
        use base64::Engine;
        if base64::engine::general_purpose::STANDARD
            .decode(&challenge.nonce)
            .ok()
            .is_none_or(|n| n.len() != 32)
        {
            return Err(SyncError::Transport("invalid peer challenge".into()));
        }
        let target = format!("/v1{path}");
        let proof = crate::http_auth::Proof::create(
            &self.keys,
            &self.space,
            method,
            &target,
            challenge.nonce,
            body,
        );
        let request = self
            .agent
            .request(method, &format!("{}{path}", self.base))
            .set(crate::http_auth::AUTH_HEADER, &proof.header());
        let result = if method == "GET" || method == "HEAD" {
            request.call()
        } else {
            request.send_bytes(body)
        };
        let response = match result {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(error) => return Err(transport_error(error)),
        };
        let status = response.status();
        let tag = response
            .header(crate::http_auth::RESPONSE_HEADER)
            .unwrap_or("")
            .to_string();
        let content_limit = if (200..300).contains(&status) {
            limit
        } else {
            MAX_METADATA
        };
        if response
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok())
            .is_some_and(|n| n > content_limit)
        {
            return Err(SyncError::Transport(
                "peer response exceeds its size limit".into(),
            ));
        }
        let bytes = read_limited(response.into_reader(), content_limit)?;
        if !proof.verify_response(
            &tag,
            &self.key,
            (&self.space, method, &target),
            status,
            &bytes,
        ) {
            return Err(SyncError::Transport(
                "unverified peer response; update both devices or check pairing".into(),
            ));
        }
        Ok((status, bytes))
    }

    fn get(&self, path: &str, limit: u64, optional: bool) -> Result<Option<Vec<u8>>> {
        let (status, bytes) = self.exchange("GET", path, &[], limit)?;
        if status == 404 && optional {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return Err(SyncError::Transport(format!("host answered HTTP {status}")));
        }
        Ok(Some(bytes))
    }
    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(serde_json::from_slice(
            &self
                .get(path, MAX_METADATA, false)?
                .expect("required response"),
        )?)
    }
    fn put_bytes(&self, path: &str, bytes: &[u8], limit: u64) -> Result<()> {
        if bytes.len() as u64 > limit {
            return Err(SyncError::Transport(
                "peer upload exceeds its size limit".into(),
            ));
        }
        let (status, _) = self.exchange("PUT", path, bytes, MAX_METADATA)?;
        if !(200..300).contains(&status) {
            return Err(SyncError::Transport(format!("host answered HTTP {status}")));
        }
        Ok(())
    }
    pub fn config(&self, name: &str) -> Result<Option<Vec<u8>>> {
        crate::transport::check_device(&DeviceId::from(name))?;
        self.get(&format!("/config/{name}"), MAX_METADATA, true)
    }
    pub fn roster(&self) -> Result<Roster> {
        self.get_json("/roster")
    }
}

impl BatchStore for HttpStore {
    fn devices(&self) -> Result<Vec<DeviceId>> {
        self.get_json("/devices")
    }
    fn list(&self, device: &DeviceId, after: u64) -> Result<Vec<u64>> {
        crate::transport::check_device(device)?;
        self.get_json(&format!("/batches/{}?after={after}", device.0))
    }
    fn get(&self, device: &DeviceId, seq: u64) -> Result<Vec<u8>> {
        crate::transport::check_device(device)?;
        Ok(self
            .get(&format!("/batches/{}/{seq}", device.0), MAX_BATCH, false)?
            .expect("required batch"))
    }
    fn put(&self, device: &DeviceId, seq: u64, bytes: &[u8]) -> Result<()> {
        crate::transport::check_device(device)?;
        self.put_bytes(&format!("/batches/{}/{seq}", device.0), bytes, MAX_BATCH)
    }
    fn snapshot(&self, device: &DeviceId) -> Result<Option<Vec<u8>>> {
        crate::transport::check_device(device)?;
        self.get(&format!("/snapshots/{}", device.0), MAX_SNAPSHOT, true)
    }
    fn put_snapshot(&self, device: &DeviceId, bytes: &[u8]) -> Result<()> {
        crate::transport::check_device(device)?;
        self.put_bytes(&format!("/snapshots/{}", device.0), bytes, MAX_SNAPSHOT)
    }

    fn blob_list(&self, after: Option<&str>, limit: usize) -> Result<Vec<String>> {
        if let Some(id) = after {
            crate::transport::check_blob_id(id)?;
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        #[derive(serde::Deserialize)]
        struct BlobPage {
            ids: Vec<String>,
            #[serde(default)]
            next: Option<String>,
        }
        let mut out = Vec::new();
        let mut cursor = after.map(str::to_string);
        while out.len() < limit {
            let page_limit = (limit - out.len()).min(MAX_BLOB_LIST_LIMIT);
            let mut path = format!("/blobs?limit={page_limit}");
            if let Some(after) = cursor.as_deref() {
                path.push_str(&format!("&after={after}"));
            }
            let page: BlobPage = self.get_json(&path)?;
            for id in page.ids {
                crate::transport::check_blob_id(&id)?;
                out.push(id);
                if out.len() == limit {
                    return Ok(out);
                }
            }
            match page.next {
                Some(next) => {
                    crate::transport::check_blob_id(&next)?;
                    cursor = Some(next);
                }
                None => break,
            }
        }
        Ok(out)
    }

    fn blob_has(&self, id: &str) -> Result<bool> {
        crate::transport::check_blob_id(id)?;
        let (status, _) = self.exchange("HEAD", &format!("/blobs/{id}"), &[], MAX_METADATA)?;
        if status == 404 {
            return Ok(false);
        }
        if !(200..300).contains(&status) {
            return Err(SyncError::Transport(format!("host answered HTTP {status}")));
        }
        Ok(true)
    }

    fn blob_get(&self, id: &str) -> Result<Vec<u8>> {
        crate::transport::check_blob_id(id)?;
        Ok(self
            .get(&format!("/blobs/{id}"), MAX_BLOB_BODY, false)?
            .expect("required blob"))
    }

    fn blob_put(&self, id: &str, sealed: &[u8]) -> Result<()> {
        crate::transport::check_blob_id(id)?;
        self.put_bytes(&format!("/blobs/{id}"), sealed, MAX_BLOB_BODY)
    }
}

/// Pairs with the host at `host` using the code it shows.
pub fn pair_with(host: &str, code: &str, keys: &DeviceKeys, name: &str) -> Result<Joined> {
    let request = pairing::request(code, &keys.public(name))?;
    let response = agent()
        .post(&format!("{}/pair", base_url(host)?))
        .set("Content-Type", "application/json")
        .send_bytes(&serde_json::to_vec(&request)?)
        .map_err(transport_error)?;
    let response: PairResponse = serde_json::from_slice(&read_response(response, MAX_METADATA)?)?;
    pairing::accept(code, keys, &response)
}

#[cfg(test)]
mod security_tests {
    use super::*;

    #[test]
    fn peer_origins_preserve_loopback_tailnet_and_https_and_reject_unsafe_addresses() {
        for host in [
            "127.0.0.1:7810",
            "http://[::1]:7810",
            "100.64.0.1:7810",
            "http://mac.tailnet.ts.net:7810",
            "https://sync.example.com",
        ] {
            assert!(base_url(host).is_ok(), "{host}");
        }
        for host in [
            "http://example.com",
            "http://192.168.1.5:7810",
            "http://user:secret@127.0.0.1",
            "https://example.com/path",
            "https://example.com?token=secret",
            "https://example.com/#fragment",
            "file:///tmp/a",
            "http://localhost/a\nHost:evil",
            "http://100.128.0.1",
        ] {
            assert!(base_url(host).is_err(), "{host}");
        }
    }

    #[test]
    fn response_limits_reject_overflow_instead_of_truncating() {
        assert_eq!(read_limited(&b"1234"[..], 4).unwrap(), b"1234");
        assert!(read_limited(&b"12345"[..], 4).is_err());
        let response = ureq::Response::new(200, "OK", "12345").unwrap();
        assert!(read_response(response, 4).is_err());
        assert!(check_status(ureq::Response::new(302, "Found", "").unwrap()).is_err());
    }

    #[test]
    fn errors_do_not_echo_remote_bodies_or_credentials() {
        let response = ureq::Response::new(401, "Denied", "sensitive-token").unwrap();
        let error = transport_error(ureq::Error::Status(401, response)).to_string();
        assert!(error.contains("401"));
        assert!(!error.contains("sensitive-token"));
    }
}
