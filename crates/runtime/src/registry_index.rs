//! Format-one catalog verification. Catalog authority never grants package consent.
use std::collections::BTreeSet;

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{capabilities, Package, API_VERSION, MAX_PACKAGE_BYTES};

pub type Result<T> = std::result::Result<T, String>;
pub const MAX_INDEX_BYTES: usize = 1_048_576;
pub const MAX_SIGNATURE_BYTES: usize = 4096;
pub const MAX_INTEGER: u64 = 9_007_199_254_740_991;
pub const REGISTRY_ID: &str = "terrarium-official";
pub const CLOCK_SKEW: u64 = 300_000;
pub const DAY: u64 = 86_400_000;
pub const VERIFY_ERROR: &str = "Couldn't verify the plugin list.";
pub const EXPIRED: &str = "This list is out of date.";
pub const CLOCK_ERROR: &str = "Check your device's date and time.";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Index {
    pub format: u32,
    pub registry: String,
    pub generated_ms: u64,
    pub expires_ms: u64,
    pub sequence: u64,
    pub plugins: Vec<IndexPlugin>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub publisher: String,
    pub versions: Vec<IndexVersion>,
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IndexVersion {
    pub version: String,
    pub api: String,
    pub url: String,
    pub size: u64,
    pub digest: String,
    pub capabilities: Vec<String>,
    pub min_host: String,
    pub yanked: bool,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Detached {
    format: u32,
    key: String,
    sig: String,
}

/// Compiled policy, including revocation and a sequence floor. No server-selected origins.
#[derive(Clone, Debug)]
pub struct Config {
    pub origin: String,
    pub keys: Vec<String>,
    pub revoked: Vec<String>,
    pub minimum_sequence: u64,
}
impl Config {
    pub fn package_url(&self, id: &str, v: &IndexVersion) -> String {
        format!(
            "{}/v1/packages/{}/{}/{}.terrapkg",
            self.origin, id, v.version, v.digest
        )
    }
    pub fn validate(&self) -> Result<()> {
        let host = self.origin.strip_prefix("https://").ok_or(VERIFY_ERROR)?;
        // A canonical DNS hostname only; no alternate URL spellings or IP fixtures.
        if host.len() > 253
            || !host.contains('.')
            || host.ends_with(".example")
            || host.ends_with(".localhost")
            || host.ends_with(".local")
            || host.parse::<std::net::IpAddr>().is_ok()
            || !host.split('.').all(|part| {
                !part.is_empty()
                    && part.len() <= 63
                    && part.as_bytes()[0].is_ascii_alphanumeric()
                    && part.as_bytes()[part.len() - 1].is_ascii_alphanumeric()
                    && part
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || self.minimum_sequence == 0
            || self.minimum_sequence > MAX_INTEGER
        {
            return Err(VERIFY_ERROR.into());
        }
        for key in self.keys.iter().chain(&self.revoked) {
            public_key(key)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct HighWater {
    pub sequence: u64,
    pub hash: Option<String>,
    pub last_verified_ms: u64,
}
pub fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn message(bytes: &[u8]) -> String {
    format!("terrarium-registry-index-v1:{}", hash(bytes))
}
pub fn canonical_base64(value: &str, limit: usize) -> Result<Vec<u8>> {
    if value.len() > limit.div_ceil(3) * 4 {
        return Err(VERIFY_ERROR.into());
    }
    let bytes = B64.decode(value).map_err(|_| VERIFY_ERROR)?;
    if bytes.len() > limit || B64.encode(&bytes) != value {
        return Err(VERIFY_ERROR.into());
    }
    Ok(bytes)
}
fn public_key(value: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] = canonical_base64(value, 32)?
        .try_into()
        .map_err(|_| VERIFY_ERROR)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| VERIFY_ERROR.into())
}

// Scan strings/containers before serde parsing. This also detects escaped duplicate
// object keys without allowing Value's normal last-key-wins behavior.
pub fn strict_json<T: DeserializeOwned>(
    bytes: &[u8],
    limit: usize,
    depth_limit: usize,
) -> Result<T> {
    if bytes.len() > limit || bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return Err(VERIFY_ERROR.into());
    }
    let mut depth = 0usize;
    let mut objects: Vec<Option<BTreeSet<String>>> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' => {
                depth += 1;
                if depth > depth_limit {
                    return Err(VERIFY_ERROR.into());
                }
                objects.push(if bytes[i] == b'{' {
                    Some(BTreeSet::new())
                } else {
                    None
                });
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                objects.pop();
            }
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'"' {
                        break;
                    }
                    i += 1;
                }
                if i >= bytes.len() {
                    return Err(VERIFY_ERROR.into());
                }
                let mut next = i + 1;
                while next < bytes.len() && bytes[next].is_ascii_whitespace() {
                    next += 1;
                }
                if bytes.get(next) == Some(&b':') {
                    let key: String =
                        serde_json::from_slice(&bytes[start..=i]).map_err(|_| VERIFY_ERROR)?;
                    if let Some(Some(keys)) = objects.last_mut() {
                        if !keys.insert(key) {
                            return Err(VERIFY_ERROR.into());
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    serde_json::from_slice(bytes).map_err(|_| VERIFY_ERROR.into())
}
fn slug(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn text(s: &str, min: usize, max: usize) -> bool {
    s.len() >= min && s.len() <= max && !s.chars().any(char::is_control)
}
pub fn version(s: &str) -> Option<semver::Version> {
    if s.len() > 128 || !s.is_ascii() {
        return None;
    }
    let v = semver::Version::parse(s).ok()?;
    if [v.major, v.minor, v.patch]
        .iter()
        .any(|&n| n > u32::MAX as u64)
    {
        return None;
    }
    Some(v)
}
pub fn stable(s: &str) -> Option<semver::Version> {
    version(s).filter(|v| v.pre.is_empty() && v.build.is_empty())
}
/// Validate signed metadata independently of freshness, for publishing and stale display.
pub fn validate(bytes: &[u8], config: &Config) -> Result<Index> {
    config.validate()?;
    let index: Index = strict_json(bytes, MAX_INDEX_BYTES, 16)?;
    if index.format != 1
        || index.registry != REGISTRY_ID
        || index.sequence == 0
        || [index.sequence, index.generated_ms, index.expires_ms]
            .iter()
            .any(|&n| n > MAX_INTEGER)
        || index.expires_ms <= index.generated_ms
        || index.expires_ms - index.generated_ms > 7 * DAY
        || index.plugins.len() > 1000
    {
        return Err(VERIFY_ERROR.into());
    }
    let mut ids = BTreeSet::new();
    for p in &index.plugins {
        if !slug(&p.id, 63)
            || !ids.insert(&p.id)
            || !text(&p.name, 1, 100)
            || !text(&p.description, 0, 2000)
            || !slug(&p.icon, 64)
            || p.versions.is_empty()
            || p.versions.len() > 100
        {
            return Err(VERIFY_ERROR.into());
        }
        public_key(&p.publisher)?;
        let mut versions = BTreeSet::new();
        for v in &p.versions {
            if stable(&v.version).is_none()
                || !versions.insert(&v.version)
                || !text(&v.api, 1, 128)
                || !v.api.is_ascii()
                || version(&v.min_host).is_none()
                || v.size == 0
                || v.size > MAX_PACKAGE_BYTES
                || v.digest.len() != 64
                || !v
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                || v.url.len() > 2048
                || v.url != config.package_url(&p.id, v)
                || v.capabilities.len() > 128
                || v.capabilities.iter().any(|s| !text(s, 1, 256))
                || v.capabilities.iter().collect::<BTreeSet<_>>().len() != v.capabilities.len()
            {
                return Err(VERIFY_ERROR.into());
            }
        }
    }
    Ok(index)
}
pub fn authenticate(bytes: &[u8], sig: &[u8], config: &Config) -> Result<Index> {
    let signature: Detached = strict_json(sig, MAX_SIGNATURE_BYTES, 4)?;
    if !config.keys.contains(&signature.key) || config.revoked.contains(&signature.key) {
        return Err(VERIFY_ERROR.into());
    }
    cache_integrity(bytes, sig, config)
}
/// Checks retained bookkeeping against its original signature, including after
/// key removal. This is NOT catalog authorization: callers may only retain the
/// high-water mark. Display, staging and confirmation MUST use authenticate.
pub(crate) fn cache_integrity(bytes: &[u8], sig: &[u8], config: &Config) -> Result<Index> {
    if bytes.len() > MAX_INDEX_BYTES {
        return Err(VERIFY_ERROR.into());
    }
    let sig: Detached = strict_json(sig, MAX_SIGNATURE_BYTES, 4)?;
    if sig.format != 1 {
        return Err(VERIFY_ERROR.into());
    }
    let key = public_key(&sig.key)?;
    let signature: [u8; 64] = canonical_base64(&sig.sig, 64)?
        .try_into()
        .map_err(|_| VERIFY_ERROR)?;
    key.verify_strict(
        message(bytes).as_bytes(),
        &Signature::from_bytes(&signature),
    )
    .map_err(|_| VERIFY_ERROR)?;
    validate(bytes, config)
}
pub fn freshness(
    index: &Index,
    now: u64,
    last: &HighWater,
    config: &Config,
    bytes: &[u8],
) -> Result<()> {
    if now.saturating_add(CLOCK_SKEW) < last.last_verified_ms
        || index.generated_ms > now.saturating_add(CLOCK_SKEW)
    {
        return Err(CLOCK_ERROR.into());
    }
    if index.sequence < config.minimum_sequence.max(last.sequence)
        || (index.sequence == last.sequence
            && last.hash.as_ref().is_some_and(|h| h != &hash(bytes)))
    {
        return Err(VERIFY_ERROR.into());
    }
    if index.expires_ms <= now {
        return Err(EXPIRED.into());
    }
    Ok(())
}
pub fn verify(
    bytes: &[u8],
    sig: &[u8],
    config: &Config,
    last: &HighWater,
    now: u64,
) -> Result<Index> {
    let index = authenticate(bytes, sig, config)?;
    freshness(&index, now, last, config, bytes)?;
    Ok(index)
}
pub fn sign_index(bytes: &[u8], secret: &[u8; 32], config: &Config) -> Result<Vec<u8>> {
    validate(bytes, config)?;
    let key = SigningKey::from_bytes(secret);
    serde_json::to_vec_pretty(&Detached {
        format: 1,
        key: B64.encode(key.verifying_key().as_bytes()),
        sig: B64.encode(key.sign(message(bytes).as_bytes()).to_bytes()),
    })
    .map_err(|_| VERIFY_ERROR.into())
}
pub fn compatible(v: &IndexVersion, host: &str) -> bool {
    !v.yanked
        && matches!(v.api.as_str(), API_VERSION | crate::DECLARATIVE_API_VERSION)
        && stable(&v.version).is_some()
        && version(host)
            .zip(version(&v.min_host))
            .is_some_and(|(h, m)| h.cmp_precedence(&m).is_ge())
}
pub fn newest<'a>(p: &'a IndexPlugin, host: &str) -> Option<&'a IndexVersion> {
    p.versions
        .iter()
        .filter(|v| compatible(v, host))
        .max_by_key(|v| stable(&v.version))
}
#[derive(Clone, Debug)]
pub struct Inventory {
    pub id: String,
    pub version: String,
    pub removed: bool,
    pub shadowed_by_dev: bool,
}
pub fn updates(index: &Index, installed: &[Inventory], host: &str) -> Vec<(String, String)> {
    installed
        .iter()
        .filter(|i| !i.removed && !i.shadowed_by_dev)
        .filter_map(|i| {
            let old = stable(&i.version)?;
            let v = newest(index.plugins.iter().find(|p| p.id == i.id)?, host)?;
            (stable(&v.version)? > old).then(|| (i.id.clone(), v.version.clone()))
        })
        .collect()
}
/// Pure pre-staging guard; the parsed package remains the authority for consent.
pub fn check_package(bytes: &[u8], p: &IndexPlugin, v: &IndexVersion) -> Result<Package> {
    if bytes.len() as u64 != v.size {
        return Err("Couldn't verify this plugin. Check for plugins and try again.".into());
    }
    let package = Package::from_zip(bytes)
        .map_err(|_| "Couldn't verify this plugin. Check for plugins and try again.")?;
    let m = package.manifest();
    let actual: BTreeSet<_> = capabilities(m).into_iter().map(|c| c.id).collect();
    if package.signer() != Some(p.publisher.as_str())
        || package.digest() != v.digest
        || m.id != p.id
        || m.version != v.version
        || m.api != v.api
        || actual != v.capabilities.iter().cloned().collect()
    {
        return Err("Couldn't verify this plugin. Check for plugins and try again.".into());
    }
    Ok(package)
}

/// Revalidate pending registry consent against the current fresh snapshot.
/// The adapter holds its refresh lock through staging/confirmation.
pub fn selection_unchanged(
    index: &Index,
    registry: &str,
    sequence: u64,
    plugin: &IndexPlugin,
    version: &IndexVersion,
    host: &str,
) -> Result<()> {
    let p = index
        .plugins
        .iter()
        .find(|p| p.id == plugin.id)
        .ok_or(VERIFY_ERROR)?;
    let v = p
        .versions
        .iter()
        .find(|v| v.version == version.version)
        .ok_or(VERIFY_ERROR)?;
    if index.registry != registry
        || index.sequence < sequence
        || p.publisher != plugin.publisher
        || v != version
        || !compatible(v, host)
    {
        return Err(VERIFY_ERROR.into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "registry_index_tests.rs"]
mod tests;
