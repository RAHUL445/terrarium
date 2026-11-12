//! Host-only HTTP transport. No proxy, decoder, cookie jar or connection reuse.
use crate::{HostRules, Job, NetError};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use ureq::{ReadWrite, TlsConnector};
use url::Url;

pub const MAX_RESPONSE: usize = 2 * 1024 * 1024;
const MAX_HEADERS: usize = 32 * 1024;
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub bytes_in: u64,
}
fn error(code: &str) -> NetError {
    let message = match code {
        "blocked" => "This website address is not allowed.",
        "redirect-blocked" => "This website redirect is not allowed.",
        "too-large" => "The reply is too large.",
        "timeout" => "The website did not answer in time.",
        "invalid-response" => "The website sent an invalid reply.",
        _ => "Could not reach the website.",
    };
    NetError::new(code, message)
}
fn transport(e: &io::Error) -> NetError {
    error(
        if matches!(
            e.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ) {
            "timeout"
        } else {
            "unreachable"
        },
    )
}
fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .map(|d| d.min(Duration::from_secs(10)))
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))
}
/// Reject the entire DNS answer if any address is special-purpose.
pub fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let n = u32::from(ip);
            ![
                (0x00000000, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc01fc400, 24),
                (0xc034c100, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc0af3000, 24),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 4),
                (0xf0000000, 4),
            ]
            .iter()
            .any(|&(base, bits)| n >> (32 - bits) == base >> (32 - bits))
        }
        IpAddr::V6(ip) => {
            let n = u128::from(ip);
            n >> 125 == 1
                && ![
                    (0x2001u128 << 112, 23),
                    (0x20010db8u128 << 96, 32),
                    (0x2002u128 << 112, 16),
                    (0x2620004f8000u128 << 80, 48),
                    (0x3fff0u128 << 108, 20),
                ]
                .iter()
                .any(|&(base, bits)| n >> (128 - bits) == base >> (128 - bits))
        }
    }
}
struct Lookup {
    host: String,
    reply: mpsc::SyncSender<io::Result<Vec<SocketAddr>>>,
}
fn resolver() -> &'static mpsc::SyncSender<Lookup> {
    static POOL: OnceLock<mpsc::SyncSender<Lookup>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Lookup>(4);
        let rx = Arc::new(Mutex::new(rx));
        for _ in 0..4 {
            let rx = rx.clone();
            std::thread::spawn(move || loop {
                let task = match rx.lock().unwrap().recv() {
                    Ok(v) => v,
                    Err(_) => break,
                };
                // Fixed worker count: stuck OS DNS never creates more threads.
                let result = (task.host.as_str(), 443)
                    .to_socket_addrs()
                    .map(|a| a.take(17).collect());
                let _ = task.reply.try_send(result);
            });
        }
        tx
    })
}
fn resolve(host: &str, deadline: Instant) -> Result<Vec<SocketAddr>, NetError> {
    let (tx, rx) = mpsc::sync_channel(1);
    resolver()
        .try_send(Lookup {
            host: host.into(),
            reply: tx,
        })
        .map_err(|_| error("timeout"))?;
    let result = rx
        .recv_timeout(remaining(deadline).map_err(|e| transport(&e))?)
        .map_err(|_| error("timeout"))?
        .map_err(|e| transport(&e))?;
    vetted(result)
}
fn vetted(result: Vec<SocketAddr>) -> Result<Vec<SocketAddr>, NetError> {
    if result.is_empty() {
        return Err(error("unreachable"));
    }
    if result.len() > 16 || result.iter().any(|a| !public_address(a.ip())) {
        return Err(error("blocked"));
    }
    Ok(result)
}
/// Applies inactivity and whole-job bounds even during the TLS handshake.
#[derive(Debug)]
struct Deadline {
    inner: Box<dyn ReadWrite>,
    deadline: Instant,
}
impl Deadline {
    fn check(&self) -> io::Result<()> {
        let left = remaining(self.deadline)?;
        if let Some(socket) = self.inner.socket() {
            socket.set_read_timeout(Some(left))?;
            socket.set_write_timeout(Some(left))?;
        }
        Ok(())
    }
}
impl Read for Deadline {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.inner.read(out)
    }
}
impl Write for Deadline {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check()?;
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.check()?;
        self.inner.flush()
    }
}
impl ReadWrite for Deadline {
    fn socket(&self) -> Option<&TcpStream> {
        None
    }
}
/// Bounds plaintext *before* ureq buffers/parses it, including hidden headers,
/// informational responses, chunk framing and trailers. Never prints plaintext.
struct Bounded {
    inner: Box<dyn ReadWrite>,
    headers: usize,
    total: usize,
    prefix: Vec<u8>,
    tail: [u8; 4],
    in_headers: bool,
    too_large: Arc<std::sync::atomic::AtomicBool>,
    sent: Arc<std::sync::atomic::AtomicU64>,
    request_tail: [u8; 4],
    request_headers: bool,
}
impl std::fmt::Debug for Bounded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bounded HTTP stream")
    }
}
impl Bounded {
    fn new(inner: Box<dyn ReadWrite>, too_large: Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            inner,
            headers: 0,
            total: 0,
            prefix: Vec::new(),
            tail: [0; 4],
            in_headers: true,
            too_large,
            sent: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            request_tail: [0; 4],
            request_headers: true,
        }
    }
    fn exceeded(&self) -> io::Error {
        self.too_large
            .store(true, std::sync::atomic::Ordering::Relaxed);
        io::Error::from(io::ErrorKind::InvalidData)
    }
}
impl Read for Bounded {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        let cap = if self.in_headers {
            MAX_HEADERS.saturating_sub(self.headers)
        } else {
            (MAX_RESPONSE + MAX_HEADERS).saturating_sub(self.total)
        };
        if cap == 0 {
            return Err(self.exceeded());
        }
        let len = if self.in_headers {
            1
        } else {
            out.len().min(cap)
        };
        let n = self.inner.read(&mut out[..len])?;
        for &b in &out[..n] {
            self.total += 1;
            if self.in_headers {
                self.headers += 1;
                if self.prefix.len() < 12 {
                    self.prefix.push(b);
                }
                self.tail.rotate_left(1);
                self.tail[3] = b;
                if self.tail == *b"\r\n\r\n" {
                    if self.prefix.get(9) == Some(&b'1') {
                        self.prefix.clear();
                        self.tail = [0; 4];
                    } else {
                        self.in_headers = false;
                    }
                }
            }
        }
        Ok(n)
    }
}
impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(bytes)?;
        let mut body = 0;
        for &b in &bytes[..n] {
            if self.request_headers {
                self.request_tail.rotate_left(1);
                self.request_tail[3] = b;
                if self.request_tail == *b"\r\n\r\n" {
                    self.request_headers = false;
                }
            } else {
                body += 1;
            }
        }
        self.sent
            .fetch_add(body, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
impl ReadWrite for Bounded {
    fn socket(&self) -> Option<&TcpStream> {
        None
    }
}
#[derive(Default)]
struct Policy {
    #[cfg(test)]
    endpoint: Option<SocketAddr>,
    #[cfg(test)]
    timeout: Option<Duration>,
}
struct Connector {
    deadline: Instant,
    large: Arc<std::sync::atomic::AtomicBool>,
    sent: Arc<std::sync::atomic::AtomicU64>,
    #[cfg(test)]
    plaintext: bool,
}
impl TlsConnector for Connector {
    fn connect(
        &self,
        name: &str,
        io: Box<dyn ReadWrite>,
    ) -> Result<Box<dyn ReadWrite>, ureq::Error> {
        #[cfg(test)]
        if self.plaintext {
            let mut bounded = Bounded::new(
                Box::new(Deadline {
                    inner: io,
                    deadline: self.deadline,
                }),
                self.large.clone(),
            );
            bounded.sent = self.sent.clone();
            return Ok(Box::new(bounded));
        }
        let config = rustls::ClientConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS")
        .with_root_certificates(rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        })
        .with_no_client_auth();
        let tls = Arc::new(config).connect(
            name,
            Box::new(Deadline {
                inner: io,
                deadline: self.deadline,
            }),
        )?;
        let mut bounded = Bounded::new(tls, self.large.clone());
        bounded.sent = self.sent.clone();
        Ok(Box::new(bounded))
    }
}

type AccountUpdate<'a> = dyn FnMut(
        &crate::vault::AccountLease,
        Option<crate::vault::TokenSet>,
    ) -> Result<crate::vault::AccountLease, NetError>
    + 'a;
/// Account network work runs outside the host lock. `update` briefly reacquires
/// it to check generation/consent or store a verified refresh.
pub fn run_account(
    job: &Job,
    account: Option<crate::vault::AccountLease>,
    update: &mut AccountUpdate<'_>,
) -> Result<Response, NetError> {
    run_session(job, None, account, Some(update), &Policy::default())
}
pub fn run(job: &Job, auth: Option<&str>) -> Result<Response, NetError> {
    run_with(job, auth, &Policy::default())
}
fn run_with(job: &Job, auth: Option<&str>, policy: &Policy) -> Result<Response, NetError> {
    run_session(job, auth, None, None, policy)
}
fn run_session(
    job: &Job,
    auth: Option<&str>,
    mut account: Option<crate::vault::AccountLease>,
    mut update: Option<&mut AccountUpdate<'_>>,
    policy: &Policy,
) -> Result<Response, NetError> {
    let preparation = (|| {
        let mut state = job.state.lock().unwrap();
        state.start(&job.id)?;
        state.activity(&job.plugin, false)?;
        Ok(())
    })();
    let duration = Duration::from_secs(30);
    #[cfg(test)]
    let duration = policy.timeout.unwrap_or(duration);
    let deadline = Instant::now() + duration;
    let mut audit = Vec::new();
    let result = (|| {
        preparation?;
        if let Some(error) = &job.dispatch_error {
            return Err(error.clone());
        }
        if let Some(lease) = account.as_mut() {
            *lease = update.as_mut().ok_or_else(account_unavailable)?(lease, None)?;
            if lease
                .tokens()
                .is_some_and(|t| t.expires_ms <= crate::now_ms().saturating_add(60_000))
            {
                *lease = refresh(
                    job,
                    lease,
                    update.as_mut().ok_or_else(account_unavailable)?,
                    &mut audit,
                    policy,
                    deadline,
                )?;
                job.state.lock().unwrap().next_hop(&job.id)?;
            }
        }
        let header = account.as_ref().map(|a| a.header());
        let mut check = || {
            if let Some(lease) = &account {
                update.as_mut().ok_or_else(account_unavailable)?(lease, None)?;
            }
            Ok(())
        };
        let response = run_inner(
            job,
            header.as_deref().or(auth),
            &mut audit,
            policy,
            deadline,
            MAX_RESPONSE,
            &mut check,
        )?;
        if response.status == 401 && matches!(job.request.method.as_str(), "GET" | "HEAD") {
            if let Some(lease) = account.as_mut().filter(|a| a.tokens().is_some()) {
                *lease = refresh(
                    job,
                    lease,
                    update.as_mut().ok_or_else(account_unavailable)?,
                    &mut audit,
                    policy,
                    deadline,
                )?;
                extra_attempt(job, crate::state::RESERVE)?;
                *lease = update.as_mut().ok_or_else(account_unavailable)?(lease, None)?;
                return run_inner(
                    job,
                    Some(&lease.header()),
                    &mut audit,
                    policy,
                    deadline,
                    MAX_RESPONSE,
                    &mut || {
                        update.as_mut().ok_or_else(account_unavailable)?(lease, None).map(|_| ())
                    },
                );
            }
        }
        Ok(response)
    })();
    if audit.is_empty() {
        audit.push(crate::Audit::for_job(
            job,
            &crate::request_host(&job.request.url).unwrap_or_default(),
        ));
    }
    if let Some(last) = audit.last_mut() {
        if let Err(e) = &result {
            last.code = Some(e.code.clone());
        }
        last.ms = crate::now_ms().saturating_sub(last.at_ms);
    }
    let bytes = audit.iter().map(|a| a.bytes_in + a.bytes_out).sum();
    let mut state = job.state.lock().unwrap();
    for entry in audit {
        state.audit(entry)?;
    }
    state.complete(&job.id, bytes)?;
    result
}
pub(crate) fn oauth_exchange(
    job: &Job,
    body: String,
    deadline: Instant,
    cancel: &crate::oauth::Cancel,
    check: &mut dyn FnMut() -> Result<(), NetError>,
) -> Result<String, NetError> {
    oauth_exchange_with(job, body, deadline, cancel, &Policy::default(), check)
}
fn oauth_exchange_with(
    job: &Job,
    body: String,
    deadline: Instant,
    cancel: &crate::oauth::Cancel,
    policy: &Policy,
    check: &mut dyn FnMut() -> Result<(), NetError>,
) -> Result<String, NetError> {
    let mut job = job.clone();
    job.request.headers.insert(
        "Content-Type".into(),
        "application/x-www-form-urlencoded".into(),
    );
    job.request.body = Some(body);
    // Browser waiting is not queued HTTP work; the five-minute sign-in deadline owns it.
    job.queued_at = Instant::now();
    job.queued_ms = crate::now_ms();
    let mut audit = Vec::new();
    let result = (|| {
        cancel.check()?;
        check()?;
        job.state.lock().unwrap().start(&job.id)?;
        let response = run_inner(
            &job,
            None,
            &mut audit,
            policy,
            deadline,
            64 * 1024,
            &mut || {
                cancel.check()?;
                check()
            },
        )?;
        if response.status != 200 {
            return Err(NetError::new(
                "unauthorized",
                "Could not sign in. Try again.",
            ));
        }
        Ok(response.body)
    })();
    if audit.is_empty() {
        audit.push(crate::Audit::for_job(
            &job,
            &crate::request_host(&job.request.url).unwrap_or_default(),
        ));
    }
    let last = audit.last_mut().unwrap();
    last.ms = crate::now_ms().saturating_sub(last.at_ms);
    if let Err(e) = &result {
        last.code = Some(e.code.clone());
    }
    let bytes = audit.iter().map(|a| a.bytes_in + a.bytes_out).sum();
    let mut state = job.state.lock().unwrap();
    for entry in audit {
        state.audit(entry)?;
    }
    state.complete(&job.id, bytes)?;
    result
}
fn account_unavailable() -> NetError {
    NetError::new("unavailable", "Sign in again on this device")
}
fn extra_attempt(job: &Job, bytes: u64) -> Result<(), NetError> {
    let mut state = job.state.lock().unwrap();
    state.reserve_more(&job.id, bytes)?;
    state.next_hop(&job.id)
}
fn refresh(
    job: &Job,
    lease: &crate::vault::AccountLease,
    update: &mut &mut AccountUpdate<'_>,
    audit: &mut Vec<crate::Audit>,
    policy: &Policy,
    deadline: Instant,
) -> Result<crate::vault::AccountLease, NetError> {
    // One HTTP job per plugin is in flight, therefore also one refresh/account.
    let lease = update(lease, None)?;
    let tokens = lease
        .tokens()
        .filter(|t| !t.refresh.is_empty())
        .ok_or_else(|| NetError::new("unauthorized", "Sign in again on this device"))?;
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    form.append_pair("grant_type", "refresh_token")
        .append_pair("refresh_token", &tokens.refresh)
        .append_pair("client_id", &tokens.client_id);
    if let Some(secret) = &tokens.client_secret {
        form.append_pair("client_secret", secret);
    }
    let mut refresh_job = job.clone();
    refresh_job.request = crate::Request {
        method: "POST".into(),
        url: tokens.token_url.clone(),
        headers: [(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into(),
        )]
        .into_iter()
        .collect(),
        body: Some(form.finish()),
        auth: None,
        context: serde_json::Value::Null,
    };
    {
        let mut state = job.state.lock().unwrap();
        state.reserve_more(
            &job.id,
            (64 * 1024 + refresh_job.request.body.as_ref().map_or(0, |s| s.len())) as u64,
        )?;
        if !audit.is_empty() {
            state.next_hop(&job.id)?;
        }
    }
    let response = run_inner(
        &refresh_job,
        None,
        audit,
        policy,
        deadline,
        64 * 1024,
        &mut || update(&lease, None).map(|_| ()),
    )?;
    if response.status != 200 {
        return Err(NetError::new(
            "unauthorized",
            "Sign in again on this device",
        ));
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Reply {
        access_token: String,
        token_type: String,
        expires_in: u64,
        refresh_token: Option<String>,
        scope: Option<String>,
    }
    let reply: Reply =
        serde_json::from_str(&response.body).map_err(|_| error("invalid-response"))?;
    if !reply.token_type.eq_ignore_ascii_case("bearer")
        || !crate::vault::valid_token(&reply.access_token)
        || reply
            .refresh_token
            .as_ref()
            .is_some_and(|s| !crate::vault::valid_token(s))
        || reply.expires_in == 0
        || reply.expires_in > 366 * 24 * 3600
        || reply.scope.as_ref().is_some_and(|s| s.len() > 8192)
    {
        return Err(error("invalid-response"));
    }
    let mut next = tokens.clone();
    next.access = reply.access_token;
    if let Some(refresh) = reply.refresh_token {
        next.refresh = refresh;
    }
    next.expires_ms = crate::now_ms().saturating_add(reply.expires_in * 1000);
    update(&lease, Some(next))
}
fn run_inner(
    job: &Job,
    auth: Option<&str>,
    audit: &mut Vec<crate::Audit>,
    _policy: &Policy,
    deadline: Instant,
    response_cap: usize,
    check: &mut dyn FnMut() -> Result<(), NetError>,
) -> Result<Response, NetError> {
    let original = Url::parse(&job.request.url).map_err(|_| error("invalid-response"))?;
    audit.push(crate::Audit::for_job(
        job,
        original.host_str().ok_or_else(|| error("blocked"))?,
    ));
    if job.queued_at.elapsed() > Duration::from_secs(300)
        || crate::now_ms().saturating_sub(job.queued_ms) > 300_000
    {
        return Err(error("timeout"));
    }
    if job.request.auth.is_some() && auth.is_none() {
        return Err(NetError::new(
            "unavailable",
            "Sign in again on this device.",
        ));
    }
    let mut request = job.request.clone();
    request.validate(&job.allowed)?;
    let mut url = Url::parse(&request.url).map_err(|_| error("invalid-response"))?;
    for hop in 0..=5 {
        if hop > 0 {
            job.state.lock().unwrap().next_hop(&job.id)?;
            audit.push(crate::Audit::for_job(
                job,
                url.host_str().ok_or_else(|| error("blocked"))?,
            ));
        }
        #[cfg(test)]
        let endpoint = _policy.endpoint;
        #[cfg(not(test))]
        let endpoint: Option<SocketAddr> = None;
        let addresses = if let Some(endpoint) = endpoint {
            vec![endpoint]
        } else {
            resolve(url.host_str().ok_or_else(|| error("blocked"))?, deadline)?
        };
        let large = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pinned = addresses.clone();
        let sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(remaining(deadline).map_err(|e| transport(&e))?)
            .timeout(
                deadline
                    .checked_duration_since(Instant::now())
                    .ok_or_else(|| error("timeout"))?,
            )
            .timeout_read(Duration::from_secs(10))
            .timeout_write(Duration::from_secs(10))
            .redirects(0)
            .try_proxy_from_env(false)
            .max_idle_connections(0)
            .user_agent("Terrarium")
            .resolver(move |_: &str| Ok(pinned.clone()))
            .tls_connector(Arc::new(Connector {
                deadline,
                large: large.clone(),
                sent: sent.clone(),
                #[cfg(test)]
                plaintext: endpoint.is_some(),
            }))
            .build();
        let mut outgoing = agent
            .request(&request.method, url.as_str())
            .set("Accept-Encoding", "identity");
        for (k, v) in &request.headers {
            outgoing = outgoing.set(k, v);
        }
        if let Some(auth) = auth {
            outgoing = outgoing.set("Authorization", auth);
        }
        check()?;
        remaining(deadline).map_err(|e| transport(&e))?;
        let reply = if let Some(body) = &request.body {
            outgoing.send_bytes(body.as_bytes())
        } else {
            outgoing.call()
        };
        audit.last_mut().unwrap().bytes_out = sent.load(std::sync::atomic::Ordering::Relaxed);
        let response = match reply {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(e) => {
                if large.load(std::sync::atomic::Ordering::Relaxed) {
                    return Err(error("too-large"));
                }
                let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&e);
                while let Some(err) = source {
                    if let Some(io) = err.downcast_ref::<io::Error>() {
                        return Err(transport(io));
                    }
                    source = err.source();
                }
                return Err(error(if Instant::now() >= deadline {
                    "timeout"
                } else {
                    "unreachable"
                }));
            }
        };
        let status = response.status();
        audit.last_mut().unwrap().status = Some(status);
        if matches!(request.method.as_str(), "GET" | "HEAD")
            && [301, 302, 303, 307, 308].contains(&status)
        {
            if let Some(location) = response.header("location") {
                let next = redirect(
                    &url,
                    location,
                    &job.allowed,
                    auth.is_some() || request.auth.is_some(),
                    hop,
                )?;
                if next.origin() != url.origin() {
                    request
                        .headers
                        .retain(|k, _| k.eq_ignore_ascii_case("accept"));
                }
                audit.last_mut().unwrap().ms =
                    crate::now_ms().saturating_sub(audit.last().unwrap().at_ms);
                url = next;
                continue;
            }
        }
        if response
            .all("content-encoding")
            .iter()
            .any(|v| !v.eq_ignore_ascii_case("identity"))
        {
            return Err(error("invalid-response"));
        }
        let mut headers = BTreeMap::new();
        for name in [
            "content-type",
            "etag",
            "last-modified",
            "cache-control",
            "expires",
            "retry-after",
            "date",
            "content-language",
        ] {
            let values = response.all(name);
            if !values.is_empty() {
                headers.insert(name.to_string(), values.join(", "));
            }
        }
        let mut bytes = Vec::new();
        if request.method != "HEAD" && ![204, 304].contains(&status) {
            if let Err(e) = response
                .into_reader()
                .take((response_cap + 1) as u64)
                .read_to_end(&mut bytes)
            {
                audit.last_mut().unwrap().bytes_in = bytes.len() as u64;
                return Err(if large.load(std::sync::atomic::Ordering::Relaxed) {
                    error("too-large")
                } else {
                    transport(&e)
                });
            }
        }
        audit.last_mut().unwrap().bytes_in = bytes.len() as u64;
        if bytes.len() > response_cap {
            return Err(error("too-large"));
        }
        let bytes_in = bytes.len() as u64;
        let body = String::from_utf8(bytes).map_err(|_| error("invalid-response"))?;
        return Ok(Response {
            status,
            headers,
            body,
            bytes_in,
        });
    }
    Err(error("redirect-blocked"))
}
fn redirect(
    from: &Url,
    location: &str,
    allowed: &HostRules,
    auth: bool,
    hop: usize,
) -> Result<Url, NetError> {
    let fail = || error("redirect-blocked");
    if hop >= 5 || location.chars().any(|c| c.is_control() || c == '\\') {
        return Err(fail());
    }
    let authority = location
        .split_once("://")
        .map(|(_, rest)| rest)
        .or_else(|| location.strip_prefix("//"));
    if authority.is_some_and(|a| {
        a.split(['/', '?', '#'])
            .next()
            .is_some_and(|a| a.contains('@'))
    }) {
        return Err(fail());
    }
    let next = from.join(location).map_err(|_| fail())?;
    let r =
        crate::Request::parse(&serde_json::json!({"method":"GET","url":next.as_str()}).to_string())
            .map_err(|_| fail())?;
    r.validate(allowed).map_err(|_| fail())?;
    if auth && next.origin() != from.origin() {
        return Err(fail());
    }
    Ok(next)
}

#[cfg(test)]
pub(crate) fn oauth_test_exchange(
    job: &Job,
    body: String,
    deadline: Instant,
    cancel: &crate::oauth::Cancel,
    endpoint: SocketAddr,
) -> Result<String, NetError> {
    oauth_exchange_with(
        job,
        body,
        deadline,
        cancel,
        &Policy {
            endpoint: Some(endpoint),
            timeout: None,
        },
        &mut || Ok(()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocked_ranges_are_refused() {
        for ip in [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.255",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.1",
            "192.0.2.1",
            "192.31.196.1",
            "192.52.193.1",
            "192.88.99.1",
            "192.168.0.1",
            "192.175.48.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "fc00::1",
            "fe80::1",
            "ff00::1",
            "2001::1",
            "2001:db8::1",
            "2002::1",
            "2620:4f:8000::1",
            "3fff::1",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "100.128.0.1",
            "172.32.0.1",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
        ] {
            assert!(public_address(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn redirect_to_unlisted_host_is_blocked() {
        let rules = HostRules::new(&["example.com".into()]).unwrap();
        let from = Url::parse("https://example.com/path").unwrap();
        for target in [
            "https://other.org",
            "http://example.com",
            "https://127.0.0.1",
            "https://u@example.com",
            "https://example.com/#x",
        ] {
            assert_eq!(
                redirect(&from, target, &rules, false, 0).unwrap_err().code,
                "redirect-blocked"
            );
        }
        assert_eq!(
            redirect(&from, "../next", &rules, false, 0).unwrap().path(),
            "/next"
        );
        assert!(redirect(&from, "/", &rules, false, 5).is_err());
    }
    #[test]
    fn dns_answers_are_checked_as_a_whole_and_bounded() {
        let public = "8.8.8.8:443".parse().unwrap();
        let private = "10.0.0.1:443".parse().unwrap();
        assert_eq!(vetted(vec![public, private]).unwrap_err().code, "blocked");
        assert_eq!(vetted(vec![public; 17]).unwrap_err().code, "blocked");
        assert_eq!(vetted(Vec::new()).unwrap_err().code, "unreachable");
        assert_eq!(vetted(vec![public]).unwrap(), vec![public]);
    }
    fn local_test(
        reply: impl FnOnce(tiny_http::Request) + Send + 'static,
    ) -> (Job, Policy, std::thread::JoinHandle<()>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let endpoint = server.server_addr().to_ip().unwrap();
        let thread = std::thread::spawn(move || {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(3)) {
                reply(request);
            }
        });
        let mut net = crate::Net::new();
        let request = crate::Request::parse(
            r#"{"method":"GET","url":"https://example.com/private?secret=synthetic"}"#,
        )
        .unwrap();
        let rules = HostRules::new(&["example.com".into()]).unwrap();
        net.submit("personal", "fixture", "done", request, None, &rules)
            .unwrap();
        (
            net.take().unwrap(),
            Policy {
                endpoint: Some(endpoint),
                timeout: None,
            },
            thread,
        )
    }
    fn oauth_case(
        method: &str,
        expired: bool,
        rotate: bool,
        deny_refresh: bool,
        retry_status: u16,
    ) {
        use crate::vault::{TokenSet, Vault, VAULT_STORE_ID};
        let dir = tempfile::tempdir().unwrap();
        let mut vault = Vault::open(
            dir.path(),
            terrarium_core::StoreConfig {
                space: "personal".into(),
                plugin: VAULT_STORE_ID.into(),
                device: "fixture".into(),
                user: "fixture".into(),
            },
            "personal",
        )
        .unwrap();
        vault.set_sealer(terrarium_ai::Sealer::new("space", &[7; 32]));
        let rules = HostRules::new(&["example.com".into(), "tokens.example.org".into()]).unwrap();
        vault
            .put_oauth(
                "fixture",
                "work",
                &["example.com".into()],
                TokenSet {
                    provider: Some(crate::oauth::OAuthDef {
                        auth_url: "https://example.com/authorize".into(),
                        token_url: "https://tokens.example.org/token".into(),
                        scopes: vec!["items.read".into()],
                        api_hosts: vec!["example.com".into()],
                    }),
                    access: "synthetic-access-old".into(),
                    refresh: "synthetic-refresh-old".into(),
                    expires_ms: if expired {
                        0
                    } else {
                        crate::now_ms() + 120_000
                    },
                    token_url: "https://tokens.example.org/token".into(),
                    client_id: "fixture".into(),
                    client_secret: Some("synthetic-client".into()),
                },
                &rules,
                "signer",
            )
            .unwrap();
        let lease = vault
            .lease("fixture", "work", "example.com", &rules, "signer")
            .unwrap();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let endpoint = server.server_addr().to_ip().unwrap();
        let method_owned = method.to_string();
        let refresh_expected = expired || method == "GET";
        let server_thread = std::thread::spawn(move || {
            let count = if deny_refresh {
                if expired {
                    1
                } else {
                    2
                }
            } else if !refresh_expected {
                1
            } else if expired {
                2
            } else {
                3
            };
            for _ in 0..count {
                let mut r = server
                    .recv_timeout(Duration::from_secs(3))
                    .unwrap()
                    .expect("worker request");
                if r.url() == "/token" {
                    assert!(r.headers().iter().all(|h| !h.field.equiv("Authorization")));
                    let mut body = String::new();
                    r.as_reader().read_to_string(&mut body).unwrap();
                    let form: BTreeMap<_, _> = url::form_urlencoded::parse(body.as_bytes())
                        .into_owned()
                        .collect();
                    assert_eq!(form["grant_type"], "refresh_token");
                    assert_eq!(form["refresh_token"], "synthetic-refresh-old");
                    if deny_refresh {
                        r.respond(
                            tiny_http::Response::from_string("synthetic-secret-echo")
                                .with_status_code(400),
                        )
                        .unwrap();
                    } else {
                        let mut reply = serde_json::json!({"access_token":"synthetic-access-new","token_type":"Bearer","expires_in":3600});
                        if rotate {
                            reply["refresh_token"] = serde_json::json!("synthetic-refresh-new");
                        }
                        r.respond(tiny_http::Response::from_string(reply.to_string()))
                            .unwrap();
                    }
                } else {
                    assert_eq!(r.method().as_str(), method_owned);
                    let auth = r
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .unwrap()
                        .value
                        .as_str();
                    let status = if auth == "Bearer synthetic-access-old" {
                        401
                    } else {
                        assert_eq!(auth, "Bearer synthetic-access-new");
                        retry_status
                    };
                    r.respond(
                        tiny_http::Response::from_string("synthetic API reply")
                            .with_status_code(status),
                    )
                    .unwrap();
                }
            }
        });
        let mut net = crate::Net::open(dir.path()).unwrap();
        let request = crate::Request::parse(&serde_json::json!({"method":method,"url":"https://example.com/items","auth":{"account":"work"}}).to_string()).unwrap();
        net.submit("personal", "fixture", "done", request, None, &rules)
            .unwrap();
        let job = net.take().unwrap();
        let mut calls = 0;
        let result = run_session(
            &job,
            None,
            Some(lease),
            Some(&mut |lease, tokens| {
                calls += 1;
                assert!(vault.current(lease));
                if let Some(tokens) = tokens {
                    vault.refreshed(lease, tokens)
                } else {
                    Ok(lease.clone())
                }
            }),
            &Policy {
                endpoint: Some(endpoint),
                timeout: None,
            },
        );
        server_thread.join().unwrap();
        if deny_refresh {
            assert_eq!(result.unwrap_err().code, "unauthorized");
        } else {
            assert_eq!(
                result.unwrap().status,
                if refresh_expected { retry_status } else { 401 }
            );
        }
        assert!(calls > 0);
        let lease = vault
            .lease("fixture", "work", "example.com", &rules, "signer")
            .unwrap();
        if refresh_expected && !deny_refresh {
            assert_eq!(
                lease.tokens().unwrap().refresh,
                if rotate {
                    "synthetic-refresh-new"
                } else {
                    "synthetic-refresh-old"
                }
            );
        }
        let audit = net.activity("fixture", false).unwrap();
        let counters: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("network-counters.json")).unwrap(),
        )
        .unwrap();
        let counter = counters["counters"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap();
        assert_eq!(counter["attempts"].as_array().unwrap().len(), audit.len());
        assert_eq!(
            counter["bytes"].as_u64().unwrap(),
            audit.iter().map(|a| a.bytes_in + a.bytes_out).sum::<u64>()
        );
        let text = serde_json::to_string(&audit).unwrap();
        for secret in [
            "synthetic-access",
            "synthetic-refresh",
            "synthetic-client",
            "synthetic-secret-echo",
        ] {
            assert!(!text.contains(secret));
        }
        assert_eq!(
            audit
                .iter()
                .filter(|a| a.host == "tokens.example.org")
                .count(),
            usize::from(refresh_expected)
        );
    }
    #[test]
    fn oauth_refresh_updates_tokens() {
        oauth_case("GET", true, true, false, 200);
    }
    #[test]
    fn oauth_401_get_retries_once_and_preserves_omitted_refresh() {
        oauth_case("GET", false, false, false, 200);
    }
    #[test]
    fn oauth_401_post_does_not_refresh_or_replay() {
        oauth_case("POST", false, false, false, 200);
    }
    #[test]
    fn oauth_refresh_denial_never_replays_api_request() {
        oauth_case("GET", true, false, true, 200);
    }

    #[test]
    fn oauth_second_401_does_not_refresh_again() {
        oauth_case("GET", false, false, false, 401);
    }
    #[test]
    fn oauth_post_may_proactively_refresh_without_replaying() {
        oauth_case("POST", true, false, false, 200);
    }
    #[test]
    fn auth_cannot_follow_cross_origin_even_when_both_hosts_are_allowed() {
        let rules = HostRules::new(&["example.com".into(), "other.org".into()]).unwrap();
        assert_eq!(
            redirect(
                &Url::parse("https://example.com/").unwrap(),
                "https://other.org/",
                &rules,
                true,
                0
            )
            .unwrap_err()
            .code,
            "redirect-blocked"
        );
    }

    #[test]
    fn oversized_response_is_cut_off_as_too_large() {
        let (job, policy, thread) = local_test(|r| {
            let _ = r.respond(tiny_http::Response::from_data(vec![b'x'; MAX_RESPONSE + 1]));
        });
        assert_eq!(run_with(&job, None, &policy).unwrap_err().code, "too-large");
        thread.join().unwrap();
    }
    #[test]
    fn slow_server_times_out() {
        let (job, mut policy, thread) = local_test(|r| {
            std::thread::sleep(Duration::from_millis(150));
            let _ = r.respond(tiny_http::Response::from_string("late"));
        });
        policy.timeout = Some(Duration::from_millis(25));
        assert_eq!(run_with(&job, None, &policy).unwrap_err().code, "timeout");
        thread.join().unwrap();
    }
    #[test]
    fn only_allowed_response_headers_are_returned() {
        let (job, policy, thread) = local_test(|r| {
            let mut response = tiny_http::Response::from_string("synthetic").with_status_code(404);
            for (k, v) in [
                ("ETag", "one"),
                ("ETag", "two"),
                ("Set-Cookie", "private"),
                ("X-Secret", "private"),
            ] {
                response.add_header(tiny_http::Header::from_bytes(k, v).unwrap());
            }
            r.respond(response).unwrap();
        });
        let reply = run_with(&job, None, &policy).unwrap();
        assert_eq!(reply.status, 404);
        assert_eq!(reply.body, "synthetic");
        assert_eq!(reply.headers.get("etag").unwrap(), "one, two");
        assert!(!reply.headers.contains_key("set-cookie"));
        assert!(!reply.headers.contains_key("x-secret"));
        thread.join().unwrap();
    }
    #[test]
    fn local_redirect_to_unlisted_host_is_blocked() {
        let (job, policy, thread) = local_test(|r| {
            r.respond(tiny_http::Response::empty(302).with_header(
                tiny_http::Header::from_bytes("Location", "https://other.org/private").unwrap(),
            ))
            .unwrap();
        });
        assert_eq!(
            run_with(&job, None, &policy).unwrap_err().code,
            "redirect-blocked"
        );
        thread.join().unwrap();
    }
    #[test]
    fn invalid_text_and_compression_are_rejected() {
        for compressed in [false, true] {
            let (job, policy, thread) = local_test(move |r| {
                let mut response = tiny_http::Response::from_data(if compressed {
                    b"synthetic".to_vec()
                } else {
                    vec![0xff]
                });
                if compressed {
                    response.add_header(
                        tiny_http::Header::from_bytes("Content-Encoding", "gzip").unwrap(),
                    );
                }
                r.respond(response).unwrap();
            });
            assert_eq!(
                run_with(&job, None, &policy).unwrap_err().code,
                "invalid-response"
            );
            thread.join().unwrap();
        }
    }
    #[test]
    fn writes_return_redirect_status_without_replay() {
        let (mut job, policy, thread) = local_test(|mut r| {
            let mut body = String::new();
            r.as_reader().read_to_string(&mut body).unwrap();
            assert_eq!(body, "synthetic");
            r.respond(tiny_http::Response::empty(303).with_header(
                tiny_http::Header::from_bytes("Location", "https://other.org").unwrap(),
            ))
            .unwrap();
        });
        job.request.method = "POST".into();
        job.request.body = Some("synthetic".into());
        assert_eq!(run_with(&job, None, &policy).unwrap().status, 303);
        thread.join().unwrap();
    }
    #[test]
    fn header_guard_stops_before_buffering_oversized_hidden_headers() {
        #[derive(Debug)]
        struct Memory(std::io::Cursor<Vec<u8>>);
        impl Read for Memory {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.0.read(out)
            }
        }
        impl Write for Memory {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl ReadWrite for Memory {
            fn socket(&self) -> Option<&TcpStream> {
                None
            }
        }
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let bytes = format!(
            "HTTP/1.1 200 OK\r\nSet-Cookie: {}\r\n\r\n",
            "x".repeat(MAX_HEADERS)
        );
        let mut stream = Bounded::new(
            Box::new(Memory(std::io::Cursor::new(bytes.into_bytes()))),
            flag.clone(),
        );
        let mut received = Vec::new();
        assert!(stream.read_to_end(&mut received).is_err());
        assert_eq!(received.len(), MAX_HEADERS);
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
    }
}
