//! Device-local catalog state and bounded transport, independent of plugin locks.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};

use crate::registry_index::{self as index, Config, HighWater, Index, Result, DAY, VERIFY_ERROR};

pub const NETWORK_ERROR: &str = "Couldn't check for plugins. Try again later.";
const MAX_CACHE: usize = 1_500_000;
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    format: u32,
    registry: String,
    index_base64: Option<String>,
    signature_base64: Option<String>,
    sequence: u64,
    index_sha256: String,
    last_verified_ms: u64,
    last_successful_check_ms: u64,
}
impl Envelope {
    pub fn high_water(&self) -> HighWater {
        HighWater {
            sequence: self.sequence,
            hash: Some(self.index_sha256.clone()),
            last_verified_ms: self.last_verified_ms,
        }
    }
    pub fn clear_payload(&mut self) {
        self.index_base64 = None;
        self.signature_base64 = None;
    }
    fn check_integrity(&self, config: &Config) -> Result<()> {
        if let (Some(bytes), Some(signature)) = (&self.index_base64, &self.signature_base64) {
            let bytes = index::canonical_base64(bytes, index::MAX_INDEX_BYTES)?;
            let signature = index::canonical_base64(signature, index::MAX_SIGNATURE_BYTES)?;
            let retained = index::cache_integrity(&bytes, &signature, config)?;
            if retained.sequence != self.sequence || index::hash(&bytes) != self.index_sha256 {
                return Err(VERIFY_ERROR.into());
            }
        }
        Ok(())
    }
    /// Expired authentic entries may be displayed; they never authorize an action.
    pub fn display(&self, config: &Config, now: u64) -> Result<(Index, bool)> {
        let bytes = index::canonical_base64(
            self.index_base64.as_deref().ok_or(VERIFY_ERROR)?,
            index::MAX_INDEX_BYTES,
        )?;
        let sig = index::canonical_base64(
            self.signature_base64.as_deref().ok_or(VERIFY_ERROR)?,
            index::MAX_SIGNATURE_BYTES,
        )?;
        let parsed = index::authenticate(&bytes, &sig, config)?;
        if parsed.sequence != self.sequence || index::hash(&bytes) != self.index_sha256 {
            return Err(VERIFY_ERROR.into());
        }
        match index::freshness(&parsed, now, &self.high_water(), config, &bytes) {
            Ok(()) => Ok((parsed, true)),
            Err(e) if e == index::EXPIRED => Ok((parsed, false)),
            Err(e) => Err(e),
        }
    }
    pub fn fresh(&self, config: &Config, now: u64) -> Result<Index> {
        let (index, fresh) = self.display(config, now)?;
        if !fresh {
            return Err(index::EXPIRED.into());
        }
        Ok(index)
    }
}
fn read_bounded(path: &Path, limit: usize) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(VERIFY_ERROR.into()),
    };
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| VERIFY_ERROR)?;
    if bytes.len() > limit {
        return Err(VERIFY_ERROR.into());
    }
    Ok(Some(bytes))
}
pub fn load(path: &Path) -> Result<Option<Envelope>> {
    let Some(bytes) = read_bounded(path, MAX_CACHE)? else {
        return Ok(None);
    };
    let e: Envelope = index::strict_json(&bytes, MAX_CACHE, 4)?;
    if e.format != 1
        || e.registry != index::REGISTRY_ID
        || e.sequence == 0
        || [e.sequence, e.last_verified_ms, e.last_successful_check_ms]
            .iter()
            .any(|&n| n > index::MAX_INTEGER)
        || e.last_successful_check_ms > e.last_verified_ms
        || e.index_sha256.len() != 64
        || !e
            .index_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || e.index_base64.is_some() != e.signature_base64.is_some()
    {
        return Err(VERIFY_ERROR.into());
    }
    Ok(Some(e))
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_sync(path, bytes, |parent| {
        std::fs::File::open(parent)?.sync_all()
    })
}
fn atomic_write_with_sync(
    path: &Path,
    bytes: &[u8],
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    let parent = path.parent().ok_or(VERIFY_ERROR)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| VERIFY_ERROR)?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| VERIFY_ERROR)?;
    // Retain a durable backup until the directory commit succeeds.
    let previous = read_bounded(path, MAX_CACHE)?;
    let mut backup = tempfile::NamedTempFile::new_in(parent).map_err(|_| VERIFY_ERROR)?;
    if let Some(old) = &previous {
        backup
            .write_all(old)
            .and_then(|()| backup.as_file().sync_all())
            .map_err(|_| VERIFY_ERROR)?;
    }
    temporary.persist(path).map_err(|_| VERIFY_ERROR)?;
    if sync_directory(parent).is_err() {
        if previous.is_some() {
            let _ = backup.persist(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::File::open(parent).and_then(|dir| dir.sync_all());
        return Err(VERIFY_ERROR.into());
    }
    Ok(())
}
/// Call under the profile's refresh mutex. The latest on-disk state is compared
/// again here, after fetching, so an older response cannot win a race.
pub fn commit(
    path: &Path,
    bytes: &[u8],
    sig: &[u8],
    config: &Config,
    now: u64,
) -> Result<Envelope> {
    let previous = load(path)?;
    if let Some(e) = &previous {
        // A cleared payload retains the same rollback/clock protection.
        e.check_integrity(config)?;
    }
    let last = previous.map(|e| e.high_water()).unwrap_or_default();
    if now.saturating_add(index::CLOCK_SKEW) < last.last_verified_ms {
        return Err(index::CLOCK_ERROR.into());
    }
    let parsed = index::verify(bytes, sig, config, &last, now)?;
    let envelope = Envelope {
        format: 1,
        registry: index::REGISTRY_ID.into(),
        index_base64: Some(B64.encode(bytes)),
        signature_base64: Some(B64.encode(sig)),
        sequence: parsed.sequence,
        index_sha256: index::hash(bytes),
        last_verified_ms: now.max(last.last_verified_ms),
        last_successful_check_ms: now,
    };
    let encoded = serde_json::to_vec(&envelope).map_err(|_| VERIFY_ERROR)?;
    if encoded.len() > MAX_CACHE {
        return Err(VERIFY_ERROR.into());
    }
    atomic_write(path, &encoded)?;
    Ok(envelope)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub automatic_checks: bool,
    pub last_attempt_ms: Option<u64>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            automatic_checks: true,
            last_attempt_ms: None,
        }
    }
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        match read_bounded(path, 4096)? {
            Some(b) => index::strict_json(&b, 4096, 4),
            None => Ok(Self::default()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, &serde_json::to_vec(self).map_err(|_| VERIFY_ERROR)?)
    }
    pub fn due(&self, now: u64) -> bool {
        self.automatic_checks
            && self
                .last_attempt_ms
                .is_none_or(|last| now.saturating_sub(last) >= DAY)
    }
}
pub fn cache_path(root: &Path) -> PathBuf {
    root.join("registry-cache.json")
}

pub trait Transport {
    fn get(&self, url: &str, limit: usize, json: bool, cancel: &AtomicBool) -> Result<Vec<u8>>;
}
pub struct Https;
impl Transport for Https {
    fn get(&self, url: &str, limit: usize, json: bool, cancel: &AtomicBool) -> Result<Vec<u8>> {
        use std::net::ToSocketAddrs;
        if cancel.load(Ordering::SeqCst) {
            return Err(NETWORK_ERROR.into());
        }
        let origin = url.split("/v1/").next().ok_or(NETWORK_ERROR)?;
        let config = Config {
            origin: origin.into(),
            keys: vec![],
            revoked: vec![],
            minimum_sequence: 1,
        };
        config.validate()?;
        let host = origin
            .strip_prefix("https://")
            .ok_or(NETWORK_ERROR)?
            .to_string();
        let started = Instant::now();
        // ureq 2's built-in deadline excludes DNS. Resolve separately with a
        // bounded wait, then give ureq only those addresses and remaining time.
        let (send, recv) = std::sync::mpsc::sync_channel(1);
        let lookup = host.clone();
        std::thread::spawn(move || {
            let _ = send.send(
                (lookup.as_str(), 443)
                    .to_socket_addrs()
                    .map(|a| a.collect::<Vec<_>>()),
            );
        });
        let addresses = recv
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| NETWORK_ERROR)?
            .map_err(|_| NETWORK_ERROR)?;
        if addresses.is_empty() || addresses.iter().any(|a| !public_address(a.ip())) {
            return Err(NETWORK_ERROR.into());
        }
        let remaining = Duration::from_secs(30).saturating_sub(started.elapsed());
        if remaining.is_zero() || cancel.load(Ordering::SeqCst) {
            return Err(NETWORK_ERROR.into());
        }
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(10))
            // A stricter ten-second HTTP deadline also bounds each blocking read.
            .timeout(remaining.min(Duration::from_secs(10)))
            .resolver(move |_: &str| Ok(addresses.clone()))
            .build();
        let response = agent
            .get(url)
            .set("Accept-Encoding", "identity")
            .set(
                "Accept",
                if json {
                    "application/json"
                } else {
                    "application/octet-stream"
                },
            )
            .call()
            .map_err(|_| NETWORK_ERROR)?;
        let status = response.status();
        let encoding = response.header("Content-Encoding").map(str::to_owned);
        let length = response.header("Content-Length").map(str::to_owned);
        checked_response(
            status,
            encoding.as_deref(),
            length.as_deref(),
            response.into_reader(),
            limit,
            cancel,
            started,
        )
    }
}
fn checked_response(
    status: u16,
    encoding: Option<&str>,
    length: Option<&str>,
    reader: impl Read,
    limit: usize,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Vec<u8>> {
    if status != 200
        || encoding.is_some_and(|s| s != "identity")
        || length.is_some_and(|s| s.parse::<u64>().map_or(true, |n| n > limit as u64))
    {
        return Err(NETWORK_ERROR.into());
    }
    bounded_body(reader, limit, cancel, started)
}

fn public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(a) => {
            !(a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_documentation()
                || a.is_multicast()
                || a.octets()[0] == 0
                || a.octets()[0] >= 240
                || (a.octets()[0] == 100 && (64..=127).contains(&a.octets()[1])))
        }
        std::net::IpAddr::V6(a) => a
            .to_ipv4_mapped()
            .map(public_address_v4)
            .unwrap_or_else(|| {
                a.segments()[0] & 0xe000 == 0x2000 && a.segments()[0..2] != [0x2001, 0x0db8]
            }),
    }
}
fn public_address_v4(ip: std::net::Ipv4Addr) -> bool {
    public_address(ip.into())
}
pub fn bounded_body(
    mut reader: impl Read,
    limit: usize,
    cancel: &AtomicBool,
    started: Instant,
) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if cancel.load(Ordering::SeqCst) || started.elapsed() >= Duration::from_secs(30) {
            return Err(NETWORK_ERROR.into());
        }
        let length = buffer
            .len()
            .min(limit.saturating_add(1).saturating_sub(bytes.len()));
        if length == 0 {
            return Err(NETWORK_ERROR.into());
        }
        let n = reader
            .read(&mut buffer[..length])
            .map_err(|_| NETWORK_ERROR)?;
        if n == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..n]);
        if bytes.len() > limit {
            return Err(NETWORK_ERROR.into());
        }
    }
}
pub fn refresh(
    transport: &impl Transport,
    path: &Path,
    config: &Config,
    now: impl Fn() -> u64,
    cancel: &AtomicBool,
) -> Result<Envelope> {
    if config.keys.is_empty() {
        return Err("No plugin list is set up".into());
    }
    config.validate()?;
    // Validate existing state before making any request.
    if let Some(e) = load(path)? {
        if now().saturating_add(index::CLOCK_SKEW) < e.last_verified_ms {
            return Err(index::CLOCK_ERROR.into());
        }
        e.check_integrity(config)?;
    }
    let bytes = transport.get(
        &format!("{}/v1/index.json", config.origin),
        index::MAX_INDEX_BYTES,
        true,
        cancel,
    )?;
    let sig = transport.get(
        &format!("{}/v1/index.sig.json", config.origin),
        index::MAX_SIGNATURE_BYTES,
        true,
        cancel,
    )?;
    if cancel.load(Ordering::SeqCst) {
        return Err(NETWORK_ERROR.into());
    }
    commit(path, &bytes, &sig, config, now())
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: u64 = 1_000_000_000;
    fn fixture(sequence: u64) -> (Config, [u8; 32], Vec<u8>, Vec<u8>) {
        let (secret, key) = crate::generate_signing_key().unwrap();
        let config = Config {
            origin: "https://plugins.example.org".into(),
            keys: vec![key],
            revoked: vec![],
            minimum_sequence: 1,
        };
        let bytes=serde_json::to_vec(&serde_json::json!({"format":1,"registry":index::REGISTRY_ID,"generated_ms":NOW,"expires_ms":NOW+DAY,"sequence":sequence,"plugins":[]})).unwrap();
        let sig = index::sign_index(&bytes, &secret, &config).unwrap();
        (config, secret, bytes, sig)
    }
    #[test]
    fn cache_restart_clearing_expiry_and_revocation_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let (mut config, _, bytes, sig) = fixture(2);
        let env = commit(&path, &bytes, &sig, &config, NOW).unwrap();
        assert!(load(&path).unwrap().unwrap().fresh(&config, NOW).is_ok());
        assert!(env.fresh(&config, NOW + DAY).is_err());
        assert!(!env.display(&config, NOW + DAY).unwrap().1);
        config.revoked = config.keys.clone();
        assert!(env.fresh(&config, NOW).is_err());
        config.revoked.clear();
        let mut cleared = env.clone();
        cleared.clear_payload();
        atomic_write(&path, &serde_json::to_vec(&cleared).unwrap()).unwrap();
        assert!(load(&path).unwrap().unwrap().fresh(&config, NOW).is_err());
        assert_eq!(cleared.high_water().hash, env.high_water().hash);
        assert!(commit(&path, &bytes, &sig, &config, NOW).is_ok());
        std::fs::write(&path, b"{broken").unwrap();
        assert!(load(&path).is_err());
        assert!(commit(&path, &bytes, &sig, &config, NOW).is_err());
    }
    #[test]
    fn revoked_or_removed_key_cache_can_only_recover_to_a_current_pinned_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let (mut config, _, bytes, sig) = fixture(2);
        let old = commit(&path, &bytes, &sig, &config, NOW).unwrap();
        let (new_secret, new_key) = crate::generate_signing_key().unwrap();
        config.revoked = config.keys.clone();
        config.keys = vec![new_key];
        config.minimum_sequence = 3;
        assert!(old.fresh(&config, NOW).is_err());
        assert!(commit(&path, &bytes, &sig, &config, NOW).is_err());
        let mut replacement: index::Index = serde_json::from_slice(&bytes).unwrap();
        replacement.sequence = 3;
        let raw = serde_json::to_vec(&replacement).unwrap();
        let signature = index::sign_index(&raw, &new_secret, &config).unwrap();
        let recovered = commit(&path, &raw, &signature, &config, NOW).unwrap();
        assert_eq!(recovered.high_water().sequence, 3);
        assert!(recovered.fresh(&config, NOW).is_ok());
        assert!(commit(&path, &bytes, &sig, &config, NOW).is_err());
    }
    #[test]
    fn invalid_and_stale_responses_do_not_advance_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let (c, s, bytes, sig) = fixture(3);
        commit(&path, &bytes, &sig, &c, NOW).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(commit(&path, &bytes, b"bad", &c, NOW).is_err());
        let mut old: index::Index = serde_json::from_slice(&bytes).unwrap();
        old.sequence = 2;
        let old_bytes = serde_json::to_vec(&old).unwrap();
        let old_sig = index::sign_index(&old_bytes, &s, &c).unwrap();
        // Models a response fetched before the newer sequence was committed.
        assert!(commit(&path, &old_bytes, &old_sig, &c, NOW).is_err());
        old.sequence = 3;
        old.expires_ms += 1;
        let changed = serde_json::to_vec(&old).unwrap();
        let changed_sig = index::sign_index(&changed, &s, &c).unwrap();
        assert!(commit(&path, &changed, &changed_sig, &c, NOW).is_err());
        assert_eq!(original, std::fs::read(&path).unwrap());
        let mut envelope = load(&path).unwrap().unwrap();
        envelope.index_sha256 = "b".repeat(64);
        atomic_write(&path, &serde_json::to_vec(&envelope).unwrap()).unwrap();
        assert!(envelope.fresh(&c, NOW).is_err());
        assert!(commit(&path, &bytes, &sig, &c, NOW).is_err());
    }
    #[test]
    fn atomic_write_failure_preserves_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache");
        std::fs::write(&path, b"previous").unwrap();
        // A directory at the target cannot be replaced by a tempfile; no partial file.
        let target = dir.path().join("directory");
        std::fs::create_dir(&target).unwrap();
        assert!(atomic_write(&target, b"new").is_err());
        assert!(
            atomic_write_with_sync(&path, b"replacement", |_| Err(std::io::Error::other(
                "injected directory sync failure"
            )))
            .is_err()
        );
        assert_eq!(std::fs::read(path).unwrap(), b"previous");
        assert!(atomic_write(&dir.path().join("missing/cache"), b"new").is_err());
    }
    struct Mock {
        bodies: Vec<Vec<u8>>,
        calls: std::cell::Cell<usize>,
    }
    impl Transport for Mock {
        fn get(&self, url: &str, limit: usize, json: bool, cancel: &AtomicBool) -> Result<Vec<u8>> {
            assert!(url.starts_with("https://plugins.example.org/v1/index."));
            assert!(json);
            let n = self.calls.get();
            self.calls.set(n + 1);
            bounded_body(
                std::io::Cursor::new(self.bodies[n].clone()),
                limit,
                cancel,
                Instant::now(),
            )
        }
    }
    #[test]
    fn empty_keys_make_zero_requests_and_mixed_snapshots_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let (mut c, _, bytes, sig) = fixture(1);
        let mock = Mock {
            bodies: vec![bytes.clone(), sig],
            calls: std::cell::Cell::new(0),
        };
        let cancel = AtomicBool::new(false);
        let keys = std::mem::take(&mut c.keys);
        assert!(refresh(&mock, &path, &c, || NOW, &cancel).is_err());
        assert_eq!(mock.calls.get(), 0);
        c.keys = keys;
        assert!(refresh(&mock, &path, &c, || NOW, &cancel).is_ok());
        assert_eq!(mock.calls.get(), 2);
        let (_, _, _, wrong_sig) = fixture(1);
        let mixed = Mock {
            bodies: vec![bytes, wrong_sig],
            calls: std::cell::Cell::new(0),
        };
        let old = std::fs::read(&path).unwrap();
        assert!(refresh(&mixed, &path, &c, || NOW, &cancel).is_err());
        assert_eq!(old, std::fs::read(path).unwrap());
    }
    #[test]
    fn bounded_streams_cancellation_and_total_timeout() {
        let cancel = AtomicBool::new(false);
        assert_eq!(
            bounded_body(&b"1234"[..], 4, &cancel, Instant::now()).unwrap(),
            b"1234"
        );
        assert!(bounded_body(&b"12345"[..], 4, &cancel, Instant::now()).is_err());
        assert!(bounded_body(
            &b"1"[..],
            4,
            &cancel,
            Instant::now() - Duration::from_secs(31)
        )
        .is_err());
        cancel.store(true, Ordering::SeqCst);
        assert!(bounded_body(&b"1"[..], 4, &cancel, Instant::now()).is_err());
        for ip in [
            "127.0.0.1",
            "192.168.1.1",
            "169.254.0.1",
            "10.0.0.1",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(!public_address(ip.parse().unwrap()));
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
    }
    #[test]
    fn redirects_compression_and_declared_or_streamed_overflow_are_refused() {
        let cancel = AtomicBool::new(false);
        for status in [301, 302, 303, 304, 307, 308, 400, 500] {
            assert!(
                checked_response(status, None, None, &b""[..], 4, &cancel, Instant::now()).is_err()
            );
        }
        for encoding in ["gzip", "br", "identity, gzip"] {
            assert!(checked_response(
                200,
                Some(encoding),
                None,
                &b"1234"[..],
                4,
                &cancel,
                Instant::now()
            )
            .is_err());
        }
        for length in ["5", "-1", "oops"] {
            assert!(checked_response(
                200,
                None,
                Some(length),
                &b""[..],
                4,
                &cancel,
                Instant::now()
            )
            .is_err());
        }
        assert!(
            checked_response(200, None, None, &b"12345"[..], 4, &cancel, Instant::now()).is_err()
        );
        assert!(checked_response(
            200,
            Some("identity"),
            Some("1"),
            &b"12345"[..],
            4,
            &cancel,
            Instant::now()
        )
        .is_err());
        assert_eq!(
            checked_response(
                200,
                None,
                Some("4"),
                &b"1234"[..],
                4,
                &cancel,
                Instant::now()
            )
            .unwrap(),
            b"1234"
        );
    }
    #[test]
    fn manual_only_and_failure_throttling_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings");
        let mut s = Settings::default();
        assert!(s.due(NOW));
        s.automatic_checks = false;
        assert!(!s.due(NOW));
        s.save(&path).unwrap();
        assert!(!Settings::load(&path).unwrap().due(NOW + DAY));
        s.automatic_checks = true;
        s.last_attempt_ms = Some(NOW);
        s.save(&path).unwrap();
        let restored = Settings::load(&path).unwrap();
        assert!(!restored.due(NOW));
        assert!(!restored.due(NOW + DAY - 1));
        assert!(restored.due(NOW + DAY));
        assert!(!restored.due(NOW - 1));
    }
}
