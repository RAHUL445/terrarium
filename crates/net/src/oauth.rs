//! Desktop browser authorization. Secrets never cross the Settings result boundary.
use crate::{invalid, vault::TokenSet, HostRules, NetError};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthDef {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: Vec<String>,
    pub api_hosts: Vec<String>,
}
impl OAuthDef {
    pub fn canonical(&self, hosts: &[String]) -> Result<Self, NetError> {
        let rules = HostRules::new(hosts)?;
        let mut out = self.clone();
        for raw in [&mut out.auth_url, &mut out.token_url] {
            crate::Request::parse(&serde_json::json!({"method":"POST","url":raw}).to_string())?
                .validate(&rules)?;
            let url = Url::parse(raw).map_err(|_| invalid())?;
            let host = crate::request_host(raw).ok_or_else(invalid)?;
            if url.query().is_some() || !rules.patterns().contains(&host) {
                return Err(invalid());
            }
            *raw = url.to_string();
        }
        if out.scopes.len() > 20
            || out.scopes.iter().any(|s| {
                s.is_empty()
                    || s.len() > 256
                    || s.chars().any(|c| c.is_whitespace() || c.is_control())
            })
        {
            return Err(invalid());
        }
        out.api_hosts = crate::vault::exact_hosts(&out.api_hosts, &rules)?;
        out.scopes.sort();
        out.scopes.dedup();
        out.api_hosts.sort();
        Ok(out)
    }
    pub fn digest(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&serde_json::json!({"auth_url":self.auth_url,"token_url":self.token_url,"scopes":self.scopes,"api_hosts":self.api_hosts})).expect("provider JSON"))
        )
    }
}

#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);
impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn check(&self) -> Result<(), NetError> {
        if self.0.load(Ordering::SeqCst) {
            Err(failure("cancelled"))
        } else {
            Ok(())
        }
    }
}
fn failure(code: &str) -> NetError {
    NetError::new(
        code,
        match code {
            "timeout" => "Sign-in timed out. Try again.",
            "cancelled" => "Sign-in was cancelled.",
            _ => "Could not sign in. Try again.",
        },
    )
}
/// Provider-neutral flow configuration. No provider-supplied extra parameters.
pub struct OAuthConfig {
    pub provider: OAuthDef,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub cancel: Cancel,
}
impl std::fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuthConfig (redacted)")
    }
}
/// A reserved, audited exchange scoped to one space/plugin's existing quota state.
type ApprovalCheck = dyn Fn() -> Result<(), NetError> + Send + Sync;
pub struct Client {
    pub(crate) job: crate::Job,
    guard: Option<Box<ApprovalCheck>>,
    slots: crate::OAuthSlots,
}
impl Client {
    pub fn set_guard(&mut self, check: impl Fn() -> Result<(), NetError> + Send + Sync + 'static) {
        self.guard = Some(Box::new(check));
    }
    fn check(&self) -> Result<(), NetError> {
        self.guard.as_ref().map_or(Ok(()), |check| check())
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.job.state.lock().unwrap().cancel(&self.job.id);
        self.slots.lock().unwrap().remove(&self.job.id);
    }
}
impl crate::Net {
    pub fn oauth_client(
        &mut self,
        space: &str,
        plugin: &str,
        provider: &OAuthDef,
        rules: &HostRules,
    ) -> Result<Client, NetError> {
        let request = crate::Request::parse(
            &serde_json::json!({"method":"POST","url":provider.token_url}).to_string(),
        )?;
        request.validate(rules)?;
        let mut slots = self.oauth_slots.lock().unwrap();
        if slots.len() + self.running.len() >= 4
            || slots.values().any(|(s, p)| s == space && p == plugin)
            || self
                .running
                .iter()
                .any(|j| j.space == space && j.plugin == plugin)
        {
            return Err(NetError::new(
                "queue-full",
                "A web request is already in progress. Try again.",
            ));
        }
        let id = format!("oauth-{}", uuid::Uuid::now_v7());
        self.state.lock().unwrap().reserve(&id, space, plugin)?;
        slots.insert(id.clone(), (space.into(), plugin.into()));
        Ok(Client {
            slots: self.oauth_slots.clone(),
            guard: None,
            job: crate::Job {
                id,
                space: space.into(),
                plugin: plugin.into(),
                on_done: String::new(),
                request,
                run: None,
                queued_ms: crate::now_ms(),
                queued_at: Instant::now(),
                sequence: 0,
                allowed: rules.clone(),
                dispatch_error: None,
                state: self.state.clone(),
            },
        })
    }
}
fn random<const N: usize>() -> Result<String, NetError> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|_| failure("unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn authorize(
    cfg: &OAuthConfig,
    client: &Client,
    open: impl FnOnce(&str) -> Result<(), String>,
) -> Result<TokenSet, NetError> {
    if cfg.provider.canonical(client.job.allowed.patterns())? != cfg.provider
        || cfg.provider.token_url != client.job.request.url
    {
        return Err(invalid());
    }
    client.check()?;
    authorize_inner(cfg, open, Duration::from_secs(300), |body, deadline| {
        crate::fetch::oauth_exchange(&client.job, body, deadline, &cfg.cancel, &mut || {
            client.check()
        })
    })
}
fn authorize_inner(
    cfg: &OAuthConfig,
    open: impl FnOnce(&str) -> Result<(), String>,
    timeout: Duration,
    exchange: impl FnOnce(String, Instant) -> Result<String, NetError>,
) -> Result<TokenSet, NetError> {
    if cfg.client_id.is_empty()
        || cfg.client_id.len() > 8192
        || cfg.client_id.chars().any(char::is_control)
        || cfg
            .client_secret
            .as_ref()
            .is_some_and(|s| s.len() > 8192 || s.chars().any(char::is_control))
    {
        return Err(invalid());
    }
    cfg.cancel.check()?;
    let deadline = Instant::now() + timeout;
    let verifier = random::<32>()?;
    let state = random::<32>()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| failure("unavailable"))?;
    listener
        .set_nonblocking(true)
        .map_err(|_| failure("unavailable"))?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener
            .local_addr()
            .map_err(|_| failure("unavailable"))?
            .port()
    );
    let mut url = Url::parse(&cfg.provider.auth_url).map_err(|_| invalid())?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", &redirect)
        .append_pair("scope", &cfg.provider.scopes.join(" "))
        .append_pair("state", &state)
        .append_pair("code_challenge_method", "S256")
        .append_pair(
            "code_challenge",
            &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
        );
    open(url.as_str()).map_err(|_| failure("unavailable"))?;
    let code = loop {
        cfg.cancel.check()?;
        if Instant::now() >= deadline {
            return Err(failure("timeout"));
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let result = callback(&mut stream, &redirect, &state, deadline, &cfg.cancel);
                let text = if result.is_some() {
                    "Sign-in received. Return to Terrarium."
                } else {
                    "Unexpected request."
                };
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}", text.len(), text);
                if let Some(result) = result {
                    break result?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(_) => return Err(failure("unavailable")),
        }
    };
    drop(listener);
    cfg.cancel.check()?;
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("grant_type", "authorization_code")
        .append_pair("code", &code)
        .append_pair("redirect_uri", &redirect)
        .append_pair("code_verifier", &verifier)
        .append_pair("client_id", &cfg.client_id);
    if let Some(secret) = &cfg.client_secret {
        form.append_pair("client_secret", secret);
    }
    let body = exchange(
        form.finish(),
        deadline.min(Instant::now() + Duration::from_secs(30)),
    )?;
    cfg.cancel.check()?;
    parse_tokens(&body, cfg)
}
fn callback(
    stream: &mut TcpStream,
    redirect: &str,
    state: &str,
    deadline: Instant,
    cancel: &Cancel,
) -> Option<Result<String, NetError>> {
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    let read_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    let mut bytes = Vec::new();
    while bytes.len() < 8192 && !bytes.ends_with(b"\r\n\r\n") {
        if cancel.check().is_err() || Instant::now() >= read_deadline {
            return None;
        }
        let mut b = [0];
        match stream.read(&mut b) {
            Ok(1) => bytes.push(b[0]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue
            }
            _ => return None,
        }
    }
    if !bytes.ends_with(b"\r\n\r\n") {
        return None;
    }
    parse_callback(std::str::from_utf8(&bytes).ok()?, redirect, state)
}
fn parse_callback(request: &str, redirect: &str, state: &str) -> Option<Result<String, NetError>> {
    let mut lines = request.split("\r\n");
    let parts: Vec<_> = lines.next()?.split(' ').collect();
    if parts.len() != 3
        || parts[0] != "GET"
        || !matches!(parts[2], "HTTP/1.1" | "HTTP/1.0")
        || !parts[1].starts_with("/callback?")
    {
        return None;
    }
    let expected = Url::parse(redirect).ok()?;
    let hosts: Vec<_> = lines
        .filter_map(|l| l.split_once(':'))
        .filter(|(k, _)| k.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.trim())
        .collect();
    let authority = format!("127.0.0.1:{}", expected.port()?);
    if hosts != [authority.as_str()] {
        return None;
    }
    let url = Url::parse(&format!("http://{authority}{}", parts[1])).ok()?;
    if url.path() != "/callback" || url.fragment().is_some() {
        return None;
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let get = |key| {
        let values: Vec<_> = pairs
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.to_string())
            .collect();
        if values.len() == 1 {
            values.into_iter().next()
        } else {
            None
        }
    };
    if get("state")?.as_str() != state {
        return None;
    }
    let codes = pairs.iter().filter(|(k, _)| k == "code").count();
    let errors = pairs.iter().filter(|(k, _)| k == "error").count();
    if errors == 1 && codes == 0 {
        return Some(Err(failure("cancelled")));
    }
    if errors != 0 || codes != 1 {
        return None;
    }
    let code = get("code")?;
    if code.is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
        return None;
    }
    Some(Ok(code))
}
fn parse_tokens(body: &str, cfg: &OAuthConfig) -> Result<TokenSet, NetError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        access_token: String,
        refresh_token: Option<String>,
        token_type: String,
        expires_in: u64,
        scope: Option<String>,
    }
    if body.len() > 64 * 1024 {
        return Err(failure("invalid-response"));
    }
    let reply: Reply = serde_json::from_str(body).map_err(|_| failure("invalid-response"))?;
    if !reply.token_type.eq_ignore_ascii_case("bearer")
        || !crate::vault::valid_token(&reply.access_token)
        || reply
            .refresh_token
            .as_ref()
            .is_some_and(|s| !crate::vault::valid_token(s))
        || reply.expires_in == 0
        || reply.expires_in > 366 * 24 * 3600
        || reply
            .scope
            .is_some_and(|s| s.len() > 8192 || s.chars().any(char::is_control))
    {
        return Err(failure("invalid-response"));
    }
    Ok(TokenSet {
        access: reply.access_token,
        refresh: reply.refresh_token.unwrap_or_default(),
        expires_ms: crate::now_ms().saturating_add(reply.expires_in * 1000),
        token_url: cfg.provider.token_url.clone(),
        client_id: cfg.client_id.clone(),
        client_secret: cfg.client_secret.clone(),
        provider: Some(cfg.provider.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn provider() -> OAuthDef {
        OAuthDef {
            auth_url: "https://login.example.com/authorize".into(),
            token_url: "https://tokens.example.com/token".into(),
            scopes: vec!["items.read".into()],
            api_hosts: vec!["api.example.com".into()],
        }
    }
    fn rules() -> HostRules {
        HostRules::new(&[
            "login.example.com".into(),
            "tokens.example.com".into(),
            "api.example.com".into(),
        ])
        .unwrap()
    }
    fn config() -> OAuthConfig {
        OAuthConfig {
            provider: provider(),
            client_id: "synthetic-client".into(),
            client_secret: Some("synthetic-secret".into()),
            cancel: Cancel::default(),
        }
    }
    fn request(query: &str) -> String {
        format!("GET /callback?{query} HTTP/1.1\r\nHost: 127.0.0.1:1234\r\n\r\n")
    }
    #[test]
    fn state_mismatch_is_rejected() {
        let redirect = "http://127.0.0.1:1234/callback";
        for query in [
            "state=wrong&code=c",
            "code=c",
            "state=s&state=s&code=c",
            "state=s&code=a&code=b",
            "state=s",
            "state=s&code=",
            "state=s&code=c&error=denied",
            "state=s&error=a&error=b",
        ] {
            assert!(parse_callback(&request(query), redirect, "s").is_none());
        }
        assert_eq!(
            parse_callback(&request("state=s&code=c"), redirect, "s")
                .unwrap()
                .unwrap(),
            "c"
        );
        assert!(
            parse_callback(&request("state=s&error=denied"), redirect, "s")
                .unwrap()
                .is_err()
        );
        for bad in [
            request("state=s&code=c").replace("GET", "POST"),
            request("state=s&code=c").replace("/callback?", "/other?"),
            request("state=s&code=c").replace("127.0.0.1", "localhost"),
        ] {
            assert!(parse_callback(&bad, redirect, "s").is_none());
        }
    }
    fn browser(url: &str, bad_first: bool, deny: bool) {
        let url = Url::parse(url).unwrap();
        let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs["code_challenge_method"], "S256");
        let redirect = Url::parse(&pairs["redirect_uri"]).unwrap();
        assert_eq!(redirect.host_str(), Some("127.0.0.1"));
        let authority = format!("127.0.0.1:{}", redirect.port().unwrap());
        std::thread::spawn(move || {
            if bad_first {
                let mut s = TcpStream::connect(&authority).unwrap();
                write!(
                    s,
                    "GET /callback?state=wrong&code=stray HTTP/1.1\r\nHost: {authority}\r\n\r\n"
                )
                .unwrap();
                let mut reply = String::new();
                s.read_to_string(&mut reply).unwrap();
            }
            let mut s = TcpStream::connect(&authority).unwrap();
            let result = if deny {
                "error=denied"
            } else {
                "code=synthetic-code"
            };
            write!(
                s,
                "GET /callback?state={}&{result} HTTP/1.1\r\nHost: {authority}\r\n\r\n",
                pairs["state"]
            )
            .unwrap();
            let mut reply = String::new();
            s.read_to_string(&mut reply).unwrap();
        });
    }
    #[test]
    fn pkce_verifier_is_sent_and_tokens_are_sealed() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let endpoint = server.server_addr().to_ip().unwrap();
        let challenge = Arc::new(std::sync::Mutex::new(String::new()));
        let check = challenge.clone();
        let worker = std::thread::spawn(move || {
            let mut request = server.recv().unwrap();
            assert_eq!(request.url(), "/token");
            let mut body = String::new();
            request.as_reader().read_to_string(&mut body).unwrap();
            let form: std::collections::BTreeMap<_, _> =
                url::form_urlencoded::parse(body.as_bytes())
                    .into_owned()
                    .collect();
            assert_eq!(form["grant_type"], "authorization_code");
            assert_eq!(form["code"], "synthetic-code");
            assert_eq!(form["client_secret"], "synthetic-secret");
            assert_eq!(
                *check.lock().unwrap(),
                URL_SAFE_NO_PAD.encode(Sha256::digest(form["code_verifier"].as_bytes()))
            );
            assert_eq!(form["code_verifier"].len(), 43);
            assert!(form["redirect_uri"].starts_with("http://127.0.0.1:"));
            request.respond(tiny_http::Response::from_string(r#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600,"token_type":"Bearer"}"#)).unwrap();
        });
        let mut net = crate::Net::new();
        let client = net
            .oauth_client("personal", "fixture", &provider(), &rules())
            .unwrap();
        let cfg = config();
        let tokens = authorize_inner(
            &cfg,
            |url| {
                let url_parsed = Url::parse(url).unwrap();
                *challenge.lock().unwrap() = url_parsed
                    .query_pairs()
                    .find(|(k, _)| k == "code_challenge")
                    .unwrap()
                    .1
                    .to_string();
                browser(url, true, false);
                Ok(())
            },
            Duration::from_secs(5),
            |body, deadline| {
                crate::fetch::oauth_test_exchange(
                    &client.job,
                    body,
                    deadline,
                    &cfg.cancel,
                    endpoint,
                )
            },
        )
        .unwrap();
        worker.join().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut vault = crate::vault::Vault::open(
            dir.path(),
            terrarium_core::StoreConfig {
                device: "fixture".into(),
                space: "personal".into(),
                plugin: crate::vault::VAULT_STORE_ID.into(),
                user: "fixture".into(),
            },
            "personal",
        )
        .unwrap();
        vault.set_sealer(terrarium_ai::Sealer::new("space", &[7; 32]));
        vault
            .put_oauth(
                "fixture",
                "work",
                &provider().api_hosts,
                tokens,
                &rules(),
                "signer",
            )
            .unwrap();
        let accounts = vault.list("fixture", "signer").unwrap();
        assert_eq!(accounts[0].kind, "oauth");
        assert!(accounts[0].active);
    }
    #[test]
    fn timeout_returns_error_and_frees_port() {
        let mut port = 0;
        let error = authorize_inner(
            &config(),
            |url| {
                let url = Url::parse(url).unwrap();
                let redirect = url
                    .query_pairs()
                    .find(|(k, _)| k == "redirect_uri")
                    .unwrap()
                    .1
                    .to_string();
                port = Url::parse(&redirect).unwrap().port().unwrap();
                Ok(())
            },
            Duration::from_millis(50),
            |_, _| panic!("no exchange"),
        )
        .unwrap_err();
        assert_eq!(error.code, "timeout");
        assert!(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
    }
    #[test]
    fn cancellation_and_denial_close_listener() {
        for denial in [false, true] {
            let cfg = config();
            let mut port = 0;
            let error = authorize_inner(
                &cfg,
                |url| {
                    let urlp = Url::parse(url).unwrap();
                    let redirect = urlp
                        .query_pairs()
                        .find(|(k, _)| k == "redirect_uri")
                        .unwrap()
                        .1
                        .to_string();
                    port = Url::parse(&redirect).unwrap().port().unwrap();
                    if denial {
                        browser(url, false, true);
                    } else {
                        cfg.cancel.cancel();
                    }
                    Ok(())
                },
                Duration::from_secs(2),
                |_, _| panic!("no exchange"),
            )
            .unwrap_err();
            assert_eq!(error.code, "cancelled");
            assert!(TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).is_ok());
        }
    }
    #[test]
    fn token_error_is_reported_without_secrets() {
        for body in [
            "synthetic-secret",
            r#"{"access_token":"synthetic-access","refresh_token":"synthetic-refresh","token_type":"Basic","expires_in":3600}"#,
            r#"{"error":"synthetic-secret"}"#,
        ] {
            let error = parse_tokens(body, &config()).unwrap_err();
            assert!(!format!("{error:?}").contains("synthetic"));
        }
        assert!(!format!("{:?}", config()).contains("synthetic"));
    }
    #[test]
    fn exchange_rejects_redirects_oversize_and_secret_echo_without_leaking() {
        for (status, body) in [
            (302, "synthetic-secret".to_string()),
            (400, "synthetic-code synthetic-secret".to_string()),
            (200, "x".repeat(64 * 1024 + 1)),
        ] {
            let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
            let endpoint = server.server_addr().to_ip().unwrap();
            let worker = std::thread::spawn(move || {
                let mut request = server.recv().unwrap();
                let mut sent = String::new();
                request.as_reader().read_to_string(&mut sent).unwrap();
                assert!(sent.contains("synthetic-code"));
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(
                            tiny_http::Header::from_bytes(
                                "Location",
                                "https://tokens.example.com/elsewhere",
                            )
                            .unwrap(),
                        ),
                );
                assert!(server
                    .recv_timeout(Duration::from_millis(50))
                    .unwrap()
                    .is_none());
            });
            let dir = tempfile::tempdir().unwrap();
            let mut net = crate::Net::open(dir.path()).unwrap();
            let client = net
                .oauth_client("personal", "fixture", &provider(), &rules())
                .unwrap();
            let error = crate::fetch::oauth_test_exchange(
                &client.job,
                "code=synthetic-code&client_secret=synthetic-secret".into(),
                Instant::now() + Duration::from_secs(2),
                &Cancel::default(),
                endpoint,
            )
            .unwrap_err();
            assert!(!format!("{error:?}").contains("synthetic"));
            worker.join().unwrap();
            let audit = std::fs::read_to_string(dir.path().join("network-audit.jsonl")).unwrap();
            assert!(!audit.contains("synthetic"));
            assert_eq!(audit.lines().count(), 1);
        }
    }
    #[test]
    fn access_only_token_and_provider_binding_are_supported() {
        let cfg = config();
        let tokens = parse_tokens(
            r#"{"access_token":"synthetic-access","expires_in":3600,"token_type":"Bearer"}"#,
            &cfg,
        )
        .unwrap();
        assert!(tokens.refresh.is_empty());
        assert_eq!(tokens.provider.as_ref(), Some(&cfg.provider));
        assert!(!format!("{tokens:?}").contains("synthetic"));
    }
    #[test]
    fn sign_in_shares_http_slots_and_releases_its_reservation() {
        let mut net = crate::Net::new();
        let client = net
            .oauth_client("personal", "fixture", &provider(), &rules())
            .unwrap();
        assert!(net
            .oauth_client("personal", "fixture", &provider(), &rules())
            .is_err());
        net.submit(
            "personal",
            "fixture",
            "done",
            crate::Request::parse(r#"{"method":"GET","url":"https://api.example.com/"}"#).unwrap(),
            None,
            &rules(),
        )
        .unwrap();
        assert!(net.take().is_none());
        drop(client);
        assert!(net.take().is_some());
    }
}
