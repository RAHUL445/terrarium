//! Deferred, host-owned plugin HTTP.
pub mod fetch;
pub mod oauth;
mod state;
pub mod vault;
pub use fetch::Response;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use state::Audit;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};
use url::{Host, Url};

pub const MAX_REQUEST: usize = 384 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetError {
    pub code: String,
    pub message: String,
    pub temporary: bool,
}
impl NetError {
    fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            temporary: matches!(
                code,
                "queue-full" | "timeout" | "unreachable" | "rate-limited" | "quota"
            ),
        }
    }
}
fn invalid() -> NetError {
    NetError::new("invalid", "Invalid web request or host pattern.")
}
fn large() -> NetError {
    NetError::new("too-large", "The web request exceeds its size limit.")
}

/// Canonical host policy shared by manifests and request admission.
#[derive(Debug, Clone)]
pub struct HostRules(Vec<String>);
impl HostRules {
    pub fn new(patterns: &[String]) -> Result<Self, NetError> {
        if patterns.is_empty() || patterns.len() > 20 {
            return Err(invalid());
        }
        let mut seen = BTreeSet::new();
        let mut result = Vec::new();
        for pattern in patterns {
            let canonical = if pattern == "*" {
                if patterns.len() != 1 {
                    return Err(invalid());
                }
                "*".into()
            } else if let Some(host) = pattern.strip_prefix("*.") {
                // A wildcard may name a single label (e.g. *.com).
                format!("*.{}", canonical_host(host, true)?)
            } else {
                canonical_host(pattern, false)?
            };
            if !seen.insert(canonical.clone()) {
                return Err(invalid());
            }
            result.push(canonical);
        }
        Ok(Self(result))
    }
    pub fn patterns(&self) -> &[String] {
        &self.0
    }
    pub fn allows(&self, host: &str) -> bool {
        let Ok(host) = canonical_host(host, false) else {
            return false;
        };
        self.0.iter().any(|p| {
            p == "*"
                || p == &host
                || p.strip_prefix("*.").is_some_and(|suffix| {
                    host.strip_suffix(suffix)
                        .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
                })
        })
    }
}

fn canonical_host(raw: &str, single_label: bool) -> Result<String, NetError> {
    if raw.is_empty()
        || raw
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || ":/@\\?#%*".contains(c))
    {
        return Err(invalid());
    }
    let host = match Host::parse(raw).map_err(|_| invalid())? {
        Host::Domain(d) => d,
        _ => return Err(invalid()),
    };
    if host.len() > 253
        || (!single_label && !host.contains('.'))
        || host.split('.').any(|l| {
            l.is_empty()
                || l.len() > 63
                || l.starts_with('-')
                || l.ends_with('-')
                || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(invalid());
    }
    if [
        "localhost",
        "local",
        "internal",
        "home.arpa",
        "test",
        "invalid",
        "example",
        "onion",
    ]
    .iter()
    .any(|s| host == *s || host.ends_with(&format!(".{s}")))
    {
        return Err(invalid());
    }
    Ok(host)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthRef {
    pub account: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub method: String,
    pub url: String,
    #[serde(default, deserialize_with = "headers")]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub auth: Option<AuthRef>,
    #[serde(default)]
    pub context: Value,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP request (content redacted)")
    }
}
// Reject duplicate names before a map can discard them, including case variants.
fn headers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, String>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = BTreeMap<String, String>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unique HTTP headers")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            let mut seen = BTreeSet::new();
            while let Some((k, v)) = map.next_entry::<String, String>()? {
                if !seen.insert(k.to_ascii_lowercase()) {
                    return Err(serde::de::Error::custom("duplicate header"));
                }
                result.insert(k, v);
            }
            Ok(result)
        }
    }
    d.deserialize_map(Visitor)
}
impl Request {
    /// Errors intentionally omit parser details and input (URLs may contain secrets).
    pub fn parse(json: &str) -> Result<Self, NetError> {
        if json.len() > MAX_REQUEST {
            return Err(large());
        }
        serde_json::from_str(json).map_err(|_| invalid())
    }
    fn validate(&self, allowed: &HostRules) -> Result<(), NetError> {
        if !["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"].contains(&self.method.as_str())
            || (matches!(self.method.as_str(), "GET" | "HEAD") && self.body.is_some())
        {
            return Err(invalid());
        }
        if self.url.len() > 8192
            || self.body.as_ref().is_some_and(|b| b.len() > 256 * 1024)
            || self.headers.len() > 32
        {
            return Err(large());
        }
        if self.context.to_string().len() > 64 * 1024 {
            return Err(invalid());
        }
        if self.url.chars().any(|c| c.is_control() || c == '\\') {
            return Err(invalid());
        }
        let url = Url::parse(&self.url).map_err(|_| invalid())?;
        // Inspect authority too: an empty userinfo (`https://@host`) is forbidden.
        let authority = self
            .url
            .split_once("://")
            .ok_or_else(invalid)?
            .1
            .split(['/', '?', '#'])
            .next()
            .ok_or_else(invalid)?;
        if url.scheme() != "https"
            || authority.contains('@')
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.port_or_known_default() != Some(443)
        {
            return Err(invalid());
        }
        let host = canonical_host(url.host_str().ok_or_else(invalid)?, false)?;
        if !allowed.allows(&host) {
            return Err(NetError::new(
                "denied",
                "The website is not listed in the plugin manifest.",
            ));
        }
        let mut size = 0;
        let mut seen = BTreeSet::new();
        for (name, value) in &self.headers {
            let lower = name.to_ascii_lowercase();
            if !seen.insert(lower.clone())
                || name.is_empty()
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                || value.chars().any(char::is_control)
            {
                return Err(invalid());
            }
            let standard = [
                "accept",
                "accept-language",
                "content-type",
                "if-none-match",
                "if-modified-since",
                "idempotency-key",
            ]
            .contains(&lower.as_str());
            let custom = lower.starts_with("x-")
                && !lower.starts_with("x-forwarded-")
                && !lower.starts_with("x-auth-")
                && lower != "x-real-ip"
                && lower != "x-api-key";
            if !standard && !custom {
                return Err(invalid());
            }
            if name.len() > 4096 || value.len() > 4096 {
                return Err(large());
            }
            size += name.len() + value.len();
        }
        if size > 16 * 1024 || serde_json::to_vec(self).map_err(|_| invalid())?.len() > MAX_REQUEST
        {
            return Err(large());
        }
        if let Some(auth) = &self.auth {
            if auth.account.is_empty()
                || auth.account.len() > 60
                || !auth
                    .account
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub space: String,
    pub plugin: String,
    pub on_done: String,
    pub request: Request,
    pub run: Option<String>,
    pub queued_ms: u64,
    queued_at: std::time::Instant,
    sequence: u64,
    pub allowed: HostRules,
    /// Fixed host preflight error; delivered through the normal worker/callback path.
    pub dispatch_error: Option<NetError>,
    state: Arc<Mutex<state::State>>,
}
type OAuthSlots = Arc<Mutex<BTreeMap<String, (String, String)>>>;
#[derive(Default)]
pub struct Net {
    oauth_slots: OAuthSlots,
    queue: VecDeque<Job>,
    running: Vec<Job>,
    next_id: u64,
    dispatched: u64,
    last_dispatch: BTreeMap<(String, String), u64>,
    state: Arc<Mutex<state::State>>,
}
impl Net {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn open(dir: &std::path::Path) -> Result<Self, NetError> {
        Ok(Self {
            state: Arc::new(Mutex::new(state::State::open(dir)?)),
            ..Self::default()
        })
    }
    pub fn activity(&mut self, plugin: &str, clear: bool) -> Result<Vec<Audit>, NetError> {
        self.state.lock().unwrap().activity(plugin, clear)
    }
    /// Invalid submissions expose no unvalidated request fields to the audit.
    pub fn record_rejection(
        &mut self,
        space: &str,
        plugin: &str,
        code: &str,
    ) -> Result<(), NetError> {
        self.state.lock().unwrap().audit(Audit {
            at_ms: now_ms(),
            space: space.into(),
            plugin: plugin.into(),
            method: String::new(),
            host: String::new(),
            status: None,
            code: Some(code.into()),
            bytes_in: 0,
            bytes_out: 0,
            ms: 0,
        })
    }
    pub fn is_live(&self, id: &str) -> bool {
        self.running
            .iter()
            .any(|j| j.id == id && !j.on_done.is_empty())
    }

    pub fn available(&self, space: &str, plugin: &str) -> bool {
        self.state.lock().unwrap().capacity(space, plugin)
            && self.queue.len() < 50
            && self
                .queue
                .iter()
                .filter(|j| j.space == space && j.plugin == plugin)
                .count()
                < 20
    }
    pub fn submit(
        &mut self,
        space: &str,
        plugin: &str,
        on_done: &str,
        request: Request,
        run: Option<String>,
        allowed: &HostRules,
    ) -> Result<String, NetError> {
        request.validate(allowed)?;
        if self.queue.len() >= 50
            || self
                .queue
                .iter()
                .filter(|j| j.space == space && j.plugin == plugin)
                .count()
                >= 20
        {
            return Err(NetError::new(
                "queue-full",
                "Too many web requests are waiting.",
            ));
        }
        self.next_id += 1;
        let id = format!("http-{}", uuid::Uuid::now_v7());
        let queued_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.state.lock().unwrap().reserve(&id, space, plugin)?;
        self.queue.push_back(Job {
            id: id.clone(),
            space: space.into(),
            plugin: plugin.into(),
            on_done: on_done.into(),
            request,
            run,
            queued_ms,
            queued_at: std::time::Instant::now(),
            sequence: self.next_id,
            allowed: allowed.clone(),
            dispatch_error: None,
            state: self.state.clone(),
        });
        Ok(id)
    }
    pub fn take(&mut self) -> Option<Job> {
        let oauth = self.oauth_slots.lock().unwrap();
        if self.running.len() + oauth.len() >= 4 {
            return None;
        }
        let index = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, j)| {
                !oauth
                    .values()
                    .any(|(space, plugin)| *space == j.space && *plugin == j.plugin)
                    && !self
                        .running
                        .iter()
                        .any(|r| r.space == j.space && r.plugin == j.plugin)
            })
            .min_by_key(|(index, j)| {
                (
                    self.last_dispatch
                        .get(&(j.space.clone(), j.plugin.clone()))
                        .copied()
                        .unwrap_or(0),
                    *index,
                )
            })
            .map(|(index, _)| index)?;
        let job = self.queue.remove(index)?;
        self.dispatched += 1;
        self.last_dispatch
            .insert((job.space.clone(), job.plugin.clone()), self.dispatched);
        self.running.push(job.clone());
        Some(job)
    }
    /// Completes a taken job by identity (workers may finish out of order).
    pub fn finish(&mut self, id: &str) {
        self.state.lock().unwrap().cancel(id);
        self.running.retain(|j| j.id != id);
    }
    pub fn cancel(&mut self, id: &str) {
        self.state.lock().unwrap().cancel(id);
        self.queue.retain(|j| j.id != id);
    }
    pub fn has_run(&self, run: &str) -> bool {
        self.queue
            .iter()
            .chain(&self.running)
            .any(|j| j.run.as_deref() == Some(run))
    }
    pub fn clear(&mut self, plugin: Option<&str>) {
        self.clear_where(|j| plugin.is_none_or(|p| j.plugin == p));
    }
    pub fn clear_account(&mut self, space: &str, plugin: &str, name: &str) {
        self.clear_where(|j| {
            j.space == space
                && j.plugin == plugin
                && j.request.auth.as_ref().is_some_and(|a| a.account == name)
        });
    }
    pub fn clear_space(&mut self, space: &str) {
        self.clear_where(|j| j.space == space);
    }
    fn clear_where(&mut self, matches: impl Fn(&Job) -> bool) {
        let ids: Vec<_> = self
            .queue
            .iter()
            .filter(|j| matches(j))
            .map(|j| j.id.clone())
            .collect();
        for id in ids {
            self.cancel(&id);
        }
        // Running slots stay occupied until their worker returns; retire callback identity.
        for job in &mut self.running {
            if matches(job) {
                job.on_done.clear();
            }
        }
    }
    /// Transaction checkpoint includes jobs submitted by nested interop calls.
    pub fn checkpoint(&self) -> u64 {
        self.next_id
    }
    pub fn cancel_since(&mut self, checkpoint: u64) {
        let ids: Vec<_> = self
            .queue
            .iter()
            .filter(|j| j.sequence > checkpoint)
            .map(|j| j.id.clone())
            .collect();
        for id in ids {
            self.cancel(&id);
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP job (content redacted)")
    }
}

pub fn request_host(url: &str) -> Option<String> {
    canonical_host(Url::parse(url).ok()?.host_str()?, false).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn rules(patterns: &[&str]) -> HostRules {
        HostRules::new(&patterns.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }
    fn request(url: &str) -> Request {
        Request::parse(&json!({"method":"GET", "url":url}).to_string()).unwrap()
    }
    fn check(url: &str) -> Result<(), NetError> {
        request(url).validate(&rules(&["*"]))
    }
    #[test]
    fn sign_out_retires_only_the_named_account_in_its_space() {
        let mut net = Net::new();
        let allowed = rules(&["example.com"]);
        let mut authed = request("https://example.com");
        authed.auth = Some(AuthRef {
            account: "work".into(),
        });
        net.submit(
            "personal",
            "fixture",
            "done",
            authed.clone(),
            None,
            &allowed,
        )
        .unwrap();
        let running = net.take().unwrap();
        net.submit(
            "personal",
            "fixture",
            "done",
            request("https://example.com"),
            None,
            &allowed,
        )
        .unwrap();
        net.submit("work", "fixture", "done", authed, None, &allowed)
            .unwrap();
        net.clear_account("personal", "fixture", "work");
        assert!(!net.is_live(&running.id));
        net.finish(&running.id);
        // Fair dispatch may choose the previously idle space first.
        let remaining = [net.take().unwrap(), net.take().unwrap()];
        assert!(remaining
            .iter()
            .any(|j| j.space == "personal" && j.request.auth.is_none()));
        assert!(remaining
            .iter()
            .any(|j| j.space == "work" && j.request.auth.is_some()));
    }
    #[test]
    fn host_patterns_match_exact_and_wildcard_only() {
        let r = rules(&["EXAMPLE.COM", "*.BÜCHER.org"]);
        assert_eq!(r.patterns(), ["example.com", "*.xn--bcher-kva.org"]);
        for host in [
            "example.com",
            "a.xn--bcher-kva.org",
            "a.b.xn--bcher-kva.org",
        ] {
            assert!(r.allows(host));
        }
        for host in [
            "a.example.com",
            "xn--bcher-kva.org",
            "badxn--bcher-kva.org",
            "example.com.evil.org",
        ] {
            assert!(!r.allows(host));
        }
        assert!(r.allows("EXAMPLE.COM"));
        assert!(r.allows("a.BÜCHER.org"));
        assert!(!rules(&["*"]).allows("127.0.0.1"));
        assert!(!rules(&["*"]).allows("foo.local"));
        assert!(rules(&["*.com"]).allows("example.com"));
        for patterns in [
            vec!["*", "example.com"],
            vec!["EXAMPLE.com", "example.com"],
            vec!["bücher.org", "xn--bcher-kva.org"],
            vec!["a.*.com"],
            vec![],
        ] {
            assert!(
                HostRules::new(&patterns.into_iter().map(str::to_string).collect::<Vec<_>>())
                    .is_err()
            );
        }
        assert!(check("https://BÜCHER.org/path").is_ok());
        assert!(check("https://example.com:443/").is_ok());
    }
    #[test]
    fn ip_literals_ports_and_userinfo_are_rejected() {
        for host in [
            "127.0.0.1",
            "2130706433",
            "0177.1",
            "0x7f000001",
            "[::1]",
            "example.com:443",
            "u@example.com",
            "https://example.com",
            "example.com/path",
            "example.com.",
            "foo..com",
            "foo_bar.com",
            "-foo.com",
            "foo-.com",
            "foo.local",
            "foo.home.arpa",
            "foo.test",
            "foo.invalid",
            "foo.example",
            "foo.onion",
            "localhost",
            "internal",
        ] {
            assert!(HostRules::new(&[host.into()]).is_err(), "{host}");
        }
        for url in [
            "https://127.0.0.1",
            "https://2130706433",
            "https://0x7f000001",
            "https://[::1]",
            "https://u@example.com",
            "https://@example.com",
            "https://example.com:444",
            "http://example.com",
            "https://example.com/#",
            "https://example.com\\@other.com",
            "https://example.com/\n",
            "https://example.com.",
            "https://a.local",
        ] {
            assert!(check(url).is_err(), "{url}");
        }
    }
    #[test]
    fn forbidden_headers_are_rejected() {
        for name in [
            "Host",
            "Cookie",
            "Authorization",
            "Proxy-Authorization",
            "Forwarded",
            "Origin",
            "Referer",
            "Connection",
            "Upgrade",
            "Transfer-Encoding",
            "Content-Length",
            "Accept-Encoding",
            "TE",
            "X-Forwarded-For",
            "X-Real-IP",
            "X-Api-Key",
            "X-Auth-Token",
            "Bad Name",
        ] {
            let mut r = request("https://example.com");
            r.headers.insert(name.into(), "x".into());
            assert_eq!(
                r.validate(&rules(&["*"])).unwrap_err().code,
                "invalid",
                "{name}"
            );
        }
        for name in [
            "Accept",
            "Accept-Language",
            "Content-Type",
            "If-None-Match",
            "If-Modified-Since",
            "Idempotency-Key",
            "X-Custom",
        ] {
            let mut r = request("https://example.com");
            r.headers.insert(name.into(), "x".into());
            r.validate(&rules(&["*"])).unwrap();
            r.headers.insert(name.into(), "x\r\ny".into());
            assert!(r.validate(&rules(&["*"])).is_err());
        }
        for text in [
            r#"{"method":"GET","url":"https://example.com","headers":{"Accept":"a","accept":"b"}}"#,
            r#"{"method":"GET","url":"https://example.com","headers":{"Accept":"a","Accept":"b"}}"#,
            r#"{"method":"GET","url":"https://example.com","headers":null}"#,
        ] {
            assert!(Request::parse(text).is_err());
        }
    }
    #[test]
    fn request_shape_limits_and_private_errors() {
        for extra in [
            json!({"unknown":true}),
            json!({"auth":{"account":"a","token":"private"}}),
            json!({"auth":{"account":""}}),
            json!({"auth":{"account":"space name"}}),
            json!({"method":"get"}),
            json!({"method":"OPTIONS"}),
            json!({"body":""}),
            json!({"context":"x".repeat(64*1024)}),
        ] {
            let mut v = json!({"method":"GET","url":"https://example.com/?secret=PRIVATE"});
            v.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let error = Request::parse(&v.to_string())
                .and_then(|r| r.validate(&rules(&["*"])))
                .unwrap_err();
            assert!(!format!("{error:?}").contains("PRIVATE"));
        }
        let mut r = request("https://example.com");
        r.method = "POST".into();
        r.body = Some("x".repeat(256 * 1024));
        r.validate(&rules(&["*"])).unwrap();
        r.body.as_mut().unwrap().push('x');
        assert_eq!(r.validate(&rules(&["*"])).unwrap_err().code, "too-large");
        r.body = None;
        r.headers.insert("Accept".into(), "x".repeat(4097));
        assert_eq!(r.validate(&rules(&["*"])).unwrap_err().code, "too-large");
        assert_eq!(
            Request::parse(&" ".repeat(MAX_REQUEST + 1))
                .unwrap_err()
                .code,
            "too-large"
        );
        assert_eq!(
            request("https://other.org")
                .validate(&rules(&["example.com"]))
                .unwrap_err()
                .code,
            "denied"
        );
    }
    #[test]
    fn dispatch_is_fair_and_retired_jobs_keep_their_concurrency_slot() {
        let mut net = Net::new();
        let allowed = rules(&["*"]);
        for plugin in ["a", "a", "b", "c", "d", "e"] {
            net.submit(
                "s",
                plugin,
                "done",
                request("https://example.com"),
                None,
                &allowed,
            )
            .unwrap();
        }
        let a = net.take().unwrap();
        assert_eq!(a.plugin, "a");
        let b = net.take().unwrap();
        assert_eq!(b.plugin, "b");
        let c = net.take().unwrap();
        assert_eq!(c.plugin, "c");
        let d = net.take().unwrap();
        assert_eq!(d.plugin, "d");
        assert!(net.take().is_none());
        net.clear(Some("a"));
        assert!(!net.is_live(&a.id));
        assert!(net.take().is_none());
        net.finish(&a.id);
        assert_eq!(net.take().unwrap().plugin, "e");
    }
    #[test]
    fn queue_limit_and_cancel() {
        let mut net = Net::new();
        let allowed = rules(&["*"]);
        let mut ids = Vec::new();
        for _ in 0..20 {
            ids.push(
                net.submit(
                    "s",
                    "p",
                    "done",
                    request("https://example.com"),
                    Some("run".into()),
                    &allowed,
                )
                .unwrap(),
            );
        }
        assert_eq!(
            net.submit(
                "s",
                "p",
                "done",
                request("https://example.com"),
                None,
                &allowed
            )
            .unwrap_err()
            .code,
            "queue-full"
        );
        net.cancel(&ids[0]);
        net.submit(
            "s",
            "p",
            "done",
            request("https://example.com"),
            None,
            &allowed,
        )
        .unwrap();
        assert!(net.has_run("run"));
        let j = net.take().unwrap();
        assert!(net.take().is_none());
        net.clear(Some("p"));
        assert!(net.has_run("run"), "running jobs also hold runs");
        net.finish(&j.id);
        assert!(!net.has_run("run"));
        for i in 0..50 {
            net.submit(
                "s",
                &format!("p{i}"),
                "done",
                request("https://example.com"),
                None,
                &allowed,
            )
            .unwrap();
        }
        assert_eq!(
            net.submit(
                "s",
                "new",
                "done",
                request("https://example.com"),
                None,
                &allowed
            )
            .unwrap_err()
            .code,
            "queue-full"
        );
        let jobs: Vec<_> = (0..4).map(|_| net.take().unwrap()).collect();
        assert!(net.take().is_none());
        net.finish(&jobs[2].id);
        assert!(net.take().is_some());
        net.clear(None);
        assert!(net.queue.is_empty());
    }
    #[test]
    fn nested_checkpoints_and_unique_ids() {
        let allowed = rules(&["*"]);
        let mut net = Net::new();
        let first = net
            .submit(
                "s",
                "p",
                "done",
                request("https://example.com"),
                None,
                &allowed,
            )
            .unwrap();
        let mark = net.checkpoint();
        net.submit(
            "s",
            "child",
            "done",
            request("https://example.com"),
            None,
            &allowed,
        )
        .unwrap();
        net.cancel_since(mark);
        assert_eq!(net.queue.len(), 1);
        assert_eq!(net.queue[0].id, first);
        let second = Net::new()
            .submit(
                "s",
                "p",
                "done",
                request("https://example.com"),
                None,
                &allowed,
            )
            .unwrap();
        assert_ne!(first, second);
    }
}
