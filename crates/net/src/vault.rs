//! Synced sealed credentials and private, device-local consent. No plugin access.
use crate::{canonical_host, invalid, HostRules, NetError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use terrarium_ai::Sealer;
use terrarium_core::{PluginSchema, Store, StoreConfig, SystemTime};

pub const VAULT_STORE_ID: &str = "_vault";
const LIMIT: usize = 64 * 1024;
const SCHEMA: &str = r#"{"version":1,"tables":[{"name":"accounts","delete_policy":"delete-wins","columns":[{"name":"plugin","type":"text"},{"name":"name","type":"text"},{"name":"hosts","type":"text"},{"name":"kind","type":"text"},{"name":"sealed","type":"text"},{"name":"key_id","type":"text"},{"name":"hint","type":"text"},{"name":"expires_ms","type":"integer"},{"name":"updated_ms","type":"integer"}]}]}"#;

#[derive(Clone)]
pub struct TokenSet {
    pub provider: Option<crate::oauth::OAuthDef>,
    pub access: String,
    pub refresh: String,
    pub expires_ms: u64,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}
// Only the private sealed envelope has a serialization implementation.
mod token_wire {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        #[serde(default)]
        provider: Option<crate::oauth::OAuthDef>,
        access: String,
        refresh: String,
        expires_ms: u64,
        token_url: String,
        client_id: String,
        client_secret: Option<String>,
    }
    pub fn serialize<S: serde::Serializer>(
        value: &Option<TokenSet>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .as_ref()
            .map(|t| Wire {
                provider: t.provider.clone(),
                access: t.access.clone(),
                refresh: t.refresh.clone(),
                expires_ms: t.expires_ms,
                token_url: t.token_url.clone(),
                client_id: t.client_id.clone(),
                client_secret: t.client_secret.clone(),
            })
            .serialize(s)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<Option<TokenSet>, D::Error> {
        Ok(Option::<Wire>::deserialize(d)?.map(|t| TokenSet {
            provider: t.provider,
            access: t.access,
            refresh: t.refresh,
            expires_ms: t.expires_ms,
            token_url: t.token_url,
            client_id: t.client_id,
            client_secret: t.client_secret,
        }))
    }
}
impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSet (redacted)")
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    space: String,
    plugin: String,
    name: String,
    hosts: Vec<String>,
    kind: String,
    bearer: Option<String>,
    #[serde(with = "token_wire")]
    oauth: Option<TokenSet>,
}
#[derive(Debug, Serialize)]
pub struct AccountInfo {
    pub name: String,
    pub hosts: Vec<String>,
    pub kind: String,
    pub hint: String,
    pub readable: bool,
    pub active: bool,
    pub expires_ms: Option<u64>,
    pub provider_host: Option<String>,
    /// Exact generation displayed for approval, preventing approval races.
    pub generation: String,
}
#[derive(Clone, Deserialize, Serialize, PartialEq)]
struct Activation {
    digest: String,
    signer: String,
}
#[derive(Clone)]
pub struct AccountLease {
    envelope: Envelope,
    generation: String,
    signer: String,
}
impl std::fmt::Debug for AccountLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountLease (redacted)")
    }
}
impl AccountLease {
    pub(crate) fn header(&self) -> String {
        format!(
            "Bearer {}",
            self.envelope.bearer.as_deref().unwrap_or_else(|| &self
                .envelope
                .oauth
                .as_ref()
                .expect("validated OAuth")
                .access)
        )
    }
    pub fn is_oauth(&self) -> bool {
        self.envelope.kind == "oauth"
    }
    pub fn provider(&self) -> Option<&crate::oauth::OAuthDef> {
        self.envelope
            .oauth
            .as_ref()
            .and_then(|t| t.provider.as_ref())
    }
    pub(crate) fn tokens(&self) -> Option<&TokenSet> {
        self.envelope.oauth.as_ref()
    }
    pub fn plugin(&self) -> &str {
        &self.envelope.plugin
    }
    pub fn name(&self) -> &str {
        &self.envelope.name
    }
    pub fn space(&self) -> &str {
        &self.envelope.space
    }
}
pub struct Vault {
    store: Store,
    space: String,
    sealer: Option<Sealer>,
    approvals: BTreeMap<String, Activation>,
    local: PathBuf,
}
fn unavailable() -> NetError {
    NetError::new("unavailable", "Sign in again on this device")
}
fn storage() -> NetError {
    NetError::new("unavailable", "Could not save accounts on this device.")
}
fn id(plugin: &str, name: &str) -> String {
    json!([plugin, name]).to_string()
}
fn aad(space: &str, plugin: &str, name: &str) -> String {
    use std::fmt::Write;
    let mut out = String::from("vault:v1:");
    for part in [space, plugin, name] {
        write!(&mut out, "{}:{part}", part.len()).expect("writing into a String");
    }
    out
}
fn digest(row: &Map<String, Value>) -> String {
    // Bind all routing metadata as well as ciphertext; no peer metadata edit activates.
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(row).expect("JSON row"))
    )
}
fn text<'a>(row: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key)?.as_str()
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 60
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
pub(crate) fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 8192 && token.bytes().all(|b| b.is_ascii_graphic())
}
pub(crate) fn exact_hosts(hosts: &[String], rules: &HostRules) -> Result<Vec<String>, NetError> {
    if hosts.is_empty() || hosts.len() > 20 {
        return Err(invalid());
    }
    let mut out = Vec::new();
    for host in hosts {
        let host = canonical_host(host, false)?;
        if !rules.allows(&host) || out.contains(&host) {
            return Err(invalid());
        }
        out.push(host);
    }
    out.sort();
    Ok(out)
}
impl Vault {
    pub fn open(dir: &Path, cfg: StoreConfig, space: &str) -> Result<Self, NetError> {
        std::fs::create_dir_all(dir).map_err(|_| storage())?;
        let store = Store::open(
            dir.join("_vault.db"),
            cfg,
            serde_json::from_str::<PluginSchema>(SCHEMA).expect("vault schema"),
            Box::new(SystemTime),
        )
        .map_err(|_| storage())?;
        let local = dir.join("vault-activation.json");
        let mut bytes = Vec::new();
        match std::fs::File::open(&local) {
            Ok(f) => {
                f.take(256 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| storage())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(storage()),
        }
        if bytes.len() > 256 * 1024 {
            return Err(storage());
        }
        let approvals = if bytes.is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_slice(&bytes).map_err(|_| storage())?
        };
        Ok(Self {
            store,
            space: space.into(),
            sealer: None,
            approvals,
            local,
        })
    }
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }
    fn save_approvals(&mut self) -> Result<(), NetError> {
        // Synced deletions/replacements must not accumulate stale local consent.
        let stale: Vec<_> = self
            .approvals
            .iter()
            .filter(|(key, activation)| {
                let tuple: Option<(String, String)> = serde_json::from_str(key).ok();
                tuple
                    .and_then(|(p, n)| self.row(&p, &n).ok())
                    .is_none_or(|row| digest(&row) != activation.digest)
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in stale {
            self.approvals.remove(&key);
        }
        let bytes = serde_json::to_vec(&self.approvals).map_err(|_| storage())?;
        if bytes.len() > 256 * 1024 {
            return Err(storage());
        }
        let mut f = tempfile::NamedTempFile::new_in(self.local.parent().ok_or_else(storage)?)
            .map_err(|_| storage())?;
        f.write_all(&bytes).map_err(|_| storage())?;
        f.as_file().sync_all().map_err(|_| storage())?;
        f.persist(&self.local).map_err(|_| storage())?;
        #[cfg(unix)]
        std::fs::File::open(self.local.parent().ok_or_else(storage)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| storage())?;
        Ok(())
    }
    fn rows(&self, plugin: Option<&str>) -> Result<Vec<Map<String, Value>>, NetError> {
        let rows = self.store.query_bounded(
            "SELECT plugin, name FROM accounts WHERE _deleted IS NULL AND (?1 IS NULL OR plugin = ?1) LIMIT 201",
            &[json!(plugin)],
        ).map_err(|_| storage())?;
        rows.into_iter()
            .map(|r| {
                self.row(
                    text(&r, "plugin").ok_or_else(unavailable)?,
                    text(&r, "name").ok_or_else(unavailable)?,
                )
            })
            .collect()
    }
    fn row(&self, plugin: &str, name: &str) -> Result<Map<String, Value>, NetError> {
        let mut row = self.store.query_bounded(
            "SELECT id, plugin, name, hosts, kind, sealed, key_id, expires_ms FROM accounts WHERE id = ?1 AND _deleted IS NULL AND length(plugin) <= 128 AND length(name) <= 60 AND length(hosts) <= 8192 AND length(kind) <= 6 AND length(key_id) <= 128 AND length(CAST(sealed AS BLOB)) <= 65536",
            &[json!(id(plugin,name))],
        ).map_err(|_| storage())?.pop().ok_or_else(|| NetError::new("denied", "This account is not available."))?;
        let revision = self
            .store
            .row_revision("accounts", &id(plugin, name))
            .map_err(|_| storage())?;
        row.insert("revision".into(), json!(revision));
        Ok(row)
    }
    fn within_limits(&self, plugin: &str) -> Result<bool, NetError> {
        let counts = self.store.query_bounded("SELECT count(*) AS total, coalesce(sum(CASE WHEN plugin = ?1 THEN 1 ELSE 0 END), 0) AS own FROM accounts WHERE _deleted IS NULL", &[json!(plugin)]).map_err(|_| storage())?;
        Ok(counts.first().is_some_and(|r| {
            r["total"].as_u64().is_some_and(|n| n <= 200)
                && r["own"].as_u64().is_some_and(|n| n <= 20)
        }))
    }
    fn decrypt(&self, row: &Map<String, Value>) -> Result<Envelope, NetError> {
        let plugin = text(row, "plugin").ok_or_else(unavailable)?;
        let name = text(row, "name").ok_or_else(unavailable)?;
        if text(row, "id") != Some(id(plugin, name).as_str()) || !valid_name(name) {
            return Err(unavailable());
        }
        let sealed = text(row, "sealed")
            .filter(|s| s.len() <= LIMIT)
            .ok_or_else(unavailable)?;
        let sealer = self
            .sealer
            .as_ref()
            .filter(|s| Some(s.id()) == text(row, "key_id"))
            .ok_or_else(unavailable)?;
        let plain = sealer
            .open(&aad(&self.space, plugin, name), sealed)
            .ok_or_else(unavailable)?;
        let e: Envelope = serde_json::from_slice(&plain).map_err(|_| unavailable())?;
        if e.space != self.space
            || e.plugin != plugin
            || e.name != name
            || Some(e.kind.as_str()) != text(row, "kind")
            || Some(json!(e.hosts).to_string().as_str()) != text(row, "hosts")
            || row.get("expires_ms") != Some(&json!(e.oauth.as_ref().map(|t| t.expires_ms)))
        {
            return Err(unavailable());
        }
        if e.oauth.as_ref().is_some_and(|t| {
            t.provider.as_ref().is_some_and(|p| {
                let mut hosts = e.hosts.clone();
                hosts.sort();
                p.token_url != t.token_url || p.api_hosts != hosts
            })
        }) {
            return Err(unavailable());
        }
        match (e.kind.as_str(), &e.bearer, &e.oauth) {
            ("bearer", Some(t), None) if valid_token(t) => (),
            ("oauth", None, Some(t))
                if valid_token(&t.access) && (t.refresh.is_empty() || valid_token(&t.refresh)) => {}
            _ => return Err(unavailable()),
        }
        Ok(e)
    }
    fn active(&self, row: &Map<String, Value>, signer: &str) -> bool {
        text(row, "id")
            .and_then(|id| self.approvals.get(id))
            .is_some_and(|a| a.signer == signer && a.digest == digest(row))
    }
    /// Locally verified resealing preserves approval; unreadable ciphertext is untouched.
    pub fn set_sealer(&mut self, next: Sealer) {
        if self.sealer.as_ref().is_some_and(|s| s.id() == next.id()) {
            return;
        }
        if let Ok(rows) = self.rows(None) {
            for row in rows {
                if let Ok(e) = self.decrypt(&row) {
                    let approval = self
                        .approvals
                        .get(&id(&e.plugin, &e.name))
                        .filter(|a| a.digest == digest(&row))
                        .cloned();
                    if self.write_envelope(&e, &next).is_ok() {
                        if let (Some(mut approval), Ok(new)) =
                            (approval, self.row(&e.plugin, &e.name))
                        {
                            approval.digest = digest(&new);
                            self.approvals.insert(id(&e.plugin, &e.name), approval);
                        }
                    }
                }
            }
            if self.save_approvals().is_err() {
                self.approvals.clear();
            }
        }
        self.sealer = Some(next);
    }
    pub fn list(&self, plugin: &str, signer: &str) -> Result<Vec<AccountInfo>, NetError> {
        self.rows(Some(plugin))?
            .into_iter()
            .map(|r| {
                let e = self.decrypt(&r).ok();
                Ok(AccountInfo {
                    name: text(&r, "name").unwrap_or_default().into(),
                    hosts: e.as_ref().map(|e| e.hosts.clone()).unwrap_or_default(),
                    kind: e.as_ref().map(|e| e.kind.clone()).unwrap_or_default(),
                    hint: "Saved".into(),
                    readable: e.is_some(),
                    active: e.is_some() && self.active(&r, signer),
                    provider_host: e
                        .as_ref()
                        .and_then(|e| e.oauth.as_ref())
                        .and_then(|t| crate::request_host(&t.token_url)),
                    expires_ms: e.and_then(|e| e.oauth.map(|t| t.expires_ms)),
                    generation: digest(&r),
                })
            })
            .collect()
    }
    fn write_envelope(&mut self, e: &Envelope, sealer: &Sealer) -> Result<(), NetError> {
        let plain = serde_json::to_vec(e).map_err(|_| invalid())?;
        let sealed = sealer.seal(&aad(&e.space, &e.plugin, &e.name), &plain);
        if sealed.len() > LIMIT {
            return Err(invalid());
        }
        let values = json!({"plugin":e.plugin,"name":e.name,"hosts":json!(e.hosts).to_string(),"kind":e.kind,"sealed":sealed,"key_id":sealer.id(),"hint":"Saved","expires_ms":e.oauth.as_ref().map(|t| t.expires_ms),"updated_ms":crate::now_ms()});
        self.store
            .write(|w| {
                w.upsert(
                    "accounts",
                    &id(&e.plugin, &e.name),
                    values.as_object().expect("object"),
                )
            })
            .map_err(|_| storage())
    }
    fn put(&mut self, e: Envelope, signer: &str) -> Result<(), NetError> {
        if !valid_name(&e.name) {
            return Err(invalid());
        }
        let rows = self.rows(None)?;
        if self.row(&e.plugin, &e.name).is_err()
            && self
                .store
                .row_revision("accounts", &id(&e.plugin, &e.name))
                .map_err(|_| storage())?
                > 0
        {
            return Err(NetError::new(
                "denied",
                "This account was removed. Choose a new account name.",
            ));
        }
        if self.row(&e.plugin, &e.name).is_err()
            && (rows.len() >= 200
                || rows
                    .iter()
                    .filter(|r| text(r, "plugin") == Some(&e.plugin))
                    .count()
                    >= 20)
        {
            return Err(NetError::new("denied", "Too many saved accounts."));
        }
        let sealer = self.sealer.clone().ok_or_else(unavailable)?;
        self.write_envelope(&e, &sealer)?;
        let row = self.row(&e.plugin, &e.name)?;
        self.approve(&e.plugin, &e.name, signer, &digest(&row))
    }
    pub fn put_bearer(
        &mut self,
        plugin: &str,
        name: &str,
        hosts: &[String],
        token: &str,
        rules: &HostRules,
        signer: &str,
    ) -> Result<(), NetError> {
        if !valid_token(token) {
            return Err(invalid());
        }
        let e = Envelope {
            space: self.space.clone(),
            plugin: plugin.into(),
            name: name.into(),
            hosts: exact_hosts(hosts, rules)?,
            kind: "bearer".into(),
            bearer: Some(token.into()),
            oauth: None,
        };
        self.put(e, signer)
    }
    pub fn oauth_provider(
        &self,
        plugin: &str,
        name: &str,
    ) -> Result<Option<crate::oauth::OAuthDef>, NetError> {
        Ok(self
            .decrypt(&self.row(plugin, name)?)?
            .oauth
            .and_then(|t| t.provider))
    }
    pub fn put_oauth(
        &mut self,
        plugin: &str,
        name: &str,
        hosts: &[String],
        tokens: TokenSet,
        rules: &HostRules,
        signer: &str,
    ) -> Result<(), NetError> {
        if !valid_token(&tokens.access)
            || (!tokens.refresh.is_empty() && !valid_token(&tokens.refresh))
            || tokens.client_id.is_empty()
            || tokens.client_id.len() > 8192
            || tokens
                .client_secret
                .as_ref()
                .is_some_and(|s| s.len() > 8192)
        {
            return Err(invalid());
        }
        if let Some(provider) = &tokens.provider {
            let canonical = provider.canonical(rules.patterns())?;
            let mut bound = exact_hosts(hosts, rules)?;
            bound.sort();
            if canonical != *provider
                || provider.token_url != tokens.token_url
                || provider.api_hosts != bound
            {
                return Err(invalid());
            }
        }
        let url = url::Url::parse(&tokens.token_url).map_err(|_| invalid())?;
        let token_host = canonical_host(url.host_str().ok_or_else(invalid)?, false)?;
        let req =
            crate::Request::parse(&json!({"method":"POST","url":tokens.token_url}).to_string())?;
        req.validate(rules)?;
        if url.query().is_some() || !rules.patterns().contains(&token_host) {
            return Err(invalid());
        }
        self.put(
            Envelope {
                space: self.space.clone(),
                plugin: plugin.into(),
                name: name.into(),
                hosts: exact_hosts(hosts, rules)?,
                kind: "oauth".into(),
                bearer: None,
                oauth: Some(tokens),
            },
            signer,
        )
    }
    pub fn approve(
        &mut self,
        plugin: &str,
        name: &str,
        signer: &str,
        generation: &str,
    ) -> Result<(), NetError> {
        if !self.within_limits(plugin)? {
            return Err(NetError::new("denied", "Too many saved accounts."));
        }
        let row = self.row(plugin, name)?;
        self.decrypt(&row)?;
        if digest(&row) != generation {
            return Err(unavailable());
        }
        self.approvals.insert(
            id(plugin, name),
            Activation {
                digest: generation.into(),
                signer: signer.into(),
            },
        );
        if let Err(e) = self.save_approvals() {
            self.approvals.remove(&id(plugin, name));
            return Err(e);
        }
        Ok(())
    }
    pub fn lease(
        &self,
        plugin: &str,
        name: &str,
        host: &str,
        rules: &HostRules,
        signer: &str,
    ) -> Result<AccountLease, NetError> {
        if !self.within_limits(plugin)? {
            return Err(NetError::new("denied", "Too many saved accounts."));
        }
        let row = self.row(plugin, name)?;
        let e = self.decrypt(&row)?;
        let host = canonical_host(host, false)?;
        if !e.hosts.contains(&host)
            || exact_hosts(&e.hosts, rules).is_err()
            || e.oauth.as_ref().is_some_and(|t| {
                crate::request_host(&t.token_url).is_none_or(|h| !rules.patterns().contains(&h))
            })
        {
            return Err(NetError::new(
                "denied",
                &format!("This account is not allowed for {host}"),
            ));
        }
        if !self.active(&row, signer) {
            return Err(unavailable());
        }
        Ok(AccountLease {
            envelope: e,
            generation: digest(&row),
            signer: signer.into(),
        })
    }
    pub fn header_for(
        &self,
        plugin: &str,
        name: &str,
        host: &str,
        rules: &HostRules,
        signer: &str,
    ) -> Result<Option<String>, NetError> {
        self.lease(plugin, name, host, rules, signer)
            .map(|l| Some(l.header()))
    }
    pub fn current(&self, lease: &AccountLease) -> bool {
        self.row(lease.plugin(), lease.name()).is_ok_and(|r| {
            digest(&r) == lease.generation
                && self.active(&r, &lease.signer)
                && self.decrypt(&r).is_ok()
        })
    }
    pub fn refreshed(
        &mut self,
        lease: &AccountLease,
        tokens: TokenSet,
    ) -> Result<AccountLease, NetError> {
        if !self.current(lease) {
            return Err(unavailable());
        }
        let mut e = lease.envelope.clone();
        // Refresh may replace tokens, never routing/provider/client bindings.
        let old = e.oauth.as_ref().ok_or_else(unavailable)?;
        if !valid_token(&tokens.access)
            || (!tokens.refresh.is_empty() && !valid_token(&tokens.refresh))
            || tokens.expires_ms > i64::MAX as u64
        {
            return Err(invalid());
        }
        if tokens.token_url != old.token_url
            || tokens.client_id != old.client_id
            || tokens.client_secret != old.client_secret
            || tokens.provider != old.provider
        {
            return Err(unavailable());
        }
        e.oauth = Some(tokens);
        self.put(e, &lease.signer)?;
        let row = self.row(lease.plugin(), lease.name())?;
        Ok(AccountLease {
            envelope: self.decrypt(&row)?,
            generation: digest(&row),
            signer: lease.signer.clone(),
        })
    }
    pub fn deactivate(&mut self, lease: &AccountLease) -> Result<(), NetError> {
        if self.current(lease) {
            self.approvals.remove(&id(lease.plugin(), lease.name()));
            self.save_approvals()?;
        }
        Ok(())
    }
    pub fn remove(&mut self, plugin: &str, name: &str) -> Result<(), NetError> {
        self.approvals.remove(&id(plugin, name));
        self.save_approvals()?;
        self.store
            .write(|w| w.delete("accounts", &id(plugin, name)))
            .map_err(|_| storage())
    }
    pub fn remove_plugin(&mut self, plugin: &str) -> Result<(), NetError> {
        for row in self.rows(Some(plugin))? {
            self.remove(plugin, text(&row, "name").ok_or_else(unavailable)?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup(dir: &Path, space: &str) -> Vault {
        let cfg = StoreConfig {
            space: space.into(),
            plugin: VAULT_STORE_ID.into(),
            device: "fixture-device".into(),
            user: "fixture".into(),
        };
        let mut v = Vault::open(dir, cfg, space).unwrap();
        v.set_sealer(Sealer::new("space", &[7; 32]));
        v
    }
    fn rules() -> HostRules {
        HostRules::new(&["*.example.com".into(), "tokens.example.org".into()]).unwrap()
    }
    fn put(v: &mut Vault) {
        v.put_bearer(
            "fixture",
            "work",
            &["api.example.com".into()],
            "test-token-1234",
            &rules(),
            "signer",
        )
        .unwrap();
    }
    fn header(v: &Vault) -> Result<Option<String>, NetError> {
        v.header_for("fixture", "work", "api.example.com", &rules(), "signer")
    }
    #[test]
    fn sealed_value_is_not_plaintext_in_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        let rows = v.store.query("SELECT * FROM accounts", &[]).unwrap();
        assert!(!serde_json::to_string(&rows)
            .unwrap()
            .contains("test-token-1234"));
        assert!(
            !serde_json::to_string(&v.list("fixture", "signer").unwrap())
                .unwrap()
                .contains("1234")
        );
        assert_eq!(header(&v).unwrap().unwrap(), "Bearer test-token-1234");
        drop(v);
        assert!(!std::fs::read(dir.path().join("_vault.db"))
            .unwrap()
            .windows(15)
            .any(|s| s == b"test-token-1234"));
    }
    #[test]
    fn header_only_for_bound_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        for host in [
            "other.example.com",
            "example.com",
            "api.example.com.evil.org",
        ] {
            assert_eq!(
                v.header_for("fixture", "work", host, &rules(), "signer")
                    .unwrap_err()
                    .code,
                "denied"
            );
        }
        assert!(v
            .put_bearer(
                "fixture",
                "wild",
                &["*.example.com".into()],
                "synthetic",
                &rules(),
                "signer"
            )
            .is_err());
        assert!(v
            .put_bearer(
                "fixture",
                "unlisted",
                &["other.org".into()],
                "synthetic",
                &rules(),
                "signer"
            )
            .is_err());
        assert!(v
            .header_for(
                "fixture",
                "work",
                "api.example.com",
                &HostRules::new(&["other.org".into()]).unwrap(),
                "signer"
            )
            .is_err());
    }
    #[test]
    fn other_plugin_cannot_use_account() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        assert_eq!(
            v.header_for("other", "work", "api.example.com", &rules(), "signer")
                .unwrap_err()
                .code,
            "denied"
        );
        assert_eq!(
            v.header_for("fixture", "work", "api.example.com", &rules(), "new-signer")
                .unwrap_err()
                .code,
            "unavailable"
        );
    }
    #[test]
    fn wrong_root_reports_unreadable_not_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut v = setup(dir.path(), "personal");
            put(&mut v);
        }
        let mut v = Vault::open(
            dir.path(),
            StoreConfig {
                space: "personal".into(),
                plugin: VAULT_STORE_ID.into(),
                device: "fixture-device".into(),
                user: "fixture".into(),
            },
            "personal",
        )
        .unwrap();
        v.set_sealer(Sealer::new("space", &[8; 32]));
        assert!(!v.list("fixture", "signer").unwrap()[0].readable);
        assert_eq!(
            header(&v).unwrap_err().message,
            "Sign in again on this device"
        );
        v.set_sealer(Sealer::new("space", &[7; 32]));
        assert!(header(&v).is_ok());
    }
    #[test]
    fn root_transition_preserves_only_verified_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        v.set_sealer(Sealer::new("space", &[8; 32]));
        assert!(header(&v).is_ok());
        let row = v.row("fixture", "work").unwrap();
        assert_eq!(
            text(&row, "key_id"),
            Some(Sealer::new("space", &[8; 32]).id())
        );
        drop(v);
        let mut v = setup(dir.path(), "personal");
        assert!(header(&v).is_err());
        v.set_sealer(Sealer::new("space", &[8; 32]));
        assert!(header(&v).is_ok());
    }
    #[test]
    fn metadata_tamper_and_ciphertext_replay_require_approval() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        let original = v.row("fixture", "work").unwrap();
        for (key, value) in [
            ("hosts", json!("[\"other.example.com\"]")),
            ("kind", json!("oauth")),
            ("plugin", json!("other")),
            ("name", json!("other")),
        ] {
            v.store
                .write(|w| {
                    w.update(
                        "accounts",
                        &id("fixture", "work"),
                        &[(key.into(), value)].into_iter().collect(),
                    )
                })
                .unwrap();
            assert!(header(&v).is_err());
            v.store
                .write(|w| {
                    w.update(
                        "accounts",
                        &id("fixture", "work"),
                        &[(key.into(), original[key].clone())].into_iter().collect(),
                    )
                })
                .unwrap();
        }
        assert_eq!(header(&v).unwrap_err().code, "unavailable");
        let info = v.list("fixture", "signer").unwrap().remove(0);
        assert!(v
            .approve("fixture", "work", "signer", "stale-generation")
            .is_err());
        v.approve("fixture", "work", "signer", &info.generation)
            .unwrap();
        assert!(header(&v).is_ok());
    }
    #[test]
    fn wrong_space_and_account_aad_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        let row = v.row("fixture", "work").unwrap();
        assert!(v
            .sealer
            .as_ref()
            .unwrap()
            .open(
                &aad("personal", "other", "work"),
                text(&row, "sealed").unwrap()
            )
            .is_none());
        assert!(v
            .sealer
            .as_ref()
            .unwrap()
            .open(
                &aad("personal", "fixture", "other"),
                text(&row, "sealed").unwrap()
            )
            .is_none());
        drop(v);
        let v = setup(dir.path(), "work");
        assert!(!v.list("fixture", "signer").unwrap()[0].readable);
    }
    #[test]
    fn debug_output_has_no_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        let lease = v
            .lease("fixture", "work", "api.example.com", &rules(), "signer")
            .unwrap();
        assert!(!format!("{lease:?}").contains("test-token-1234"));
        let t = TokenSet {
            provider: None,
            access: "synthetic-access".into(),
            refresh: "synthetic-refresh".into(),
            expires_ms: 0,
            token_url: "https://tokens.example.org/token".into(),
            client_id: "fixture".into(),
            client_secret: Some("synthetic-client-secret".into()),
        };
        assert_eq!(format!("{t:?}"), "TokenSet (redacted)");
    }
    #[test]
    fn limits_removal_and_late_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        put(&mut v);
        let lease = v
            .lease("fixture", "work", "api.example.com", &rules(), "signer")
            .unwrap();
        for n in 0..19 {
            v.put_bearer(
                "fixture",
                &format!("a{n}"),
                &["api.example.com".into()],
                "synthetic",
                &rules(),
                "signer",
            )
            .unwrap();
        }
        assert!(v
            .put_bearer(
                "fixture",
                "overflow",
                &["api.example.com".into()],
                "synthetic",
                &rules(),
                "signer"
            )
            .is_err());
        v.remove_plugin("fixture").unwrap();
        assert!(v.list("fixture", "signer").unwrap().is_empty());
        assert!(!v.current(&lease));
        assert!(header(&v).is_err());
    }
    #[test]
    fn token_validation_and_local_activation() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        for bad in ["", "a b", "a\rb", "a\nb", "é"] {
            assert!(v
                .put_bearer(
                    "fixture",
                    "work",
                    &["api.example.com".into()],
                    bad,
                    &rules(),
                    "signer"
                )
                .is_err());
        }
        put(&mut v);
        v.approvals.clear();
        v.save_approvals().unwrap();
        let info = v.list("fixture", "signer").unwrap().remove(0);
        assert!(info.readable && !info.active);
        assert!(header(&v).is_err());
        v.approve("fixture", "work", "signer", &info.generation)
            .unwrap();
        assert!(header(&v).is_ok());
    }
    #[test]
    fn peer_sync_needs_local_activation_and_deletion_wins() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let mut source = setup(a.path(), "personal");
        let mut peer = setup(b.path(), "personal");
        put(&mut source);
        let (batch, cursor) = source.store.export_since(0).unwrap();
        peer.store.import(&batch).unwrap();
        assert!(peer.list("fixture", "signer").unwrap()[0].readable);
        assert!(!peer.list("fixture", "signer").unwrap()[0].active);
        assert_eq!(header(&peer).unwrap_err().code, "unavailable");
        let info = peer.list("fixture", "signer").unwrap().remove(0);
        peer.approve("fixture", "work", "signer", &info.generation)
            .unwrap();
        assert!(header(&peer).is_ok());
        source.remove_plugin("fixture").unwrap();
        let (deletion, _) = source.store.export_since(cursor).unwrap();
        peer.store.import(&deletion).unwrap();
        peer.store.import(&batch).unwrap();
        assert!(peer.list("fixture", "signer").unwrap().is_empty());
        assert!(peer
            .store
            .write(|w| w.restore("accounts", &id("fixture", "work")))
            .is_err());
        assert!(header(&peer).is_err());
    }
    #[test]
    fn late_refresh_cannot_resurrect_removed_or_replaced_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        let tokens = TokenSet {
            provider: None,
            access: "synthetic-access".into(),
            refresh: "synthetic-refresh".into(),
            expires_ms: 0,
            token_url: "https://tokens.example.org/token".into(),
            client_id: "fixture".into(),
            client_secret: None,
        };
        v.put_oauth(
            "fixture",
            "oauth",
            &["api.example.com".into()],
            tokens.clone(),
            &rules(),
            "signer",
        )
        .unwrap();
        let lease = v
            .lease("fixture", "oauth", "api.example.com", &rules(), "signer")
            .unwrap();
        v.put_bearer(
            "fixture",
            "oauth",
            &["api.example.com".into()],
            "synthetic-replacement",
            &rules(),
            "signer",
        )
        .unwrap();
        assert!(v.refreshed(&lease, tokens.clone()).is_err());
        v.remove("fixture", "oauth").unwrap();
        assert!(v.refreshed(&lease, tokens).is_err());
        assert!(v.list("fixture", "signer").unwrap().is_empty());
    }
    #[test]
    fn space_account_count_and_envelope_limits() {
        let dir = tempfile::tempdir().unwrap();
        let mut v = setup(dir.path(), "personal");
        for plugin in 0..10 {
            for account in 0..20 {
                v.put_bearer(
                    &format!("fixture{plugin}"),
                    &format!("a{account}"),
                    &["api.example.com".into()],
                    "synthetic",
                    &rules(),
                    "signer",
                )
                .unwrap();
            }
        }
        assert!(v
            .put_bearer(
                "overflow",
                "work",
                &["api.example.com".into()],
                "synthetic",
                &rules(),
                "signer"
            )
            .is_err());
        assert!(v
            .put_bearer(
                "fixture0",
                "a0",
                &["api.example.com".into()],
                &"x".repeat(8193),
                &rules(),
                "signer"
            )
            .is_err());
        // A root transition covers all accounts without one oversized query result.
        v.set_sealer(Sealer::new("space", &[8; 32]));
        assert!(v
            .list("fixture9", "signer")
            .unwrap()
            .iter()
            .all(|a| a.readable && a.active));
    }
}
