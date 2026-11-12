//! Private device-local quotas and bounded audit. Never stores request content.
use crate::{now_ms, Job, NetError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAX_FILE: u64 = 1024 * 1024;
const DAILY: u64 = 50 * 1024 * 1024;
pub(crate) const RESERVE: u64 = (crate::fetch::MAX_RESPONSE + 1 + 256 * 1024) as u64;
#[derive(Default, Serialize, Deserialize)]
struct Counter {
    attempts: Vec<u64>,
    day: u64,
    bytes: u64,
}
#[derive(Serialize, Deserialize)]
struct Reservation {
    key: String,
    at: u64,
    bytes: u64,
    started: bool,
}
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct State {
    #[serde(skip)]
    dir: Option<PathBuf>,
    clock: u64,
    counters: BTreeMap<String, Counter>,
    reservations: BTreeMap<String, Reservation>,
    #[serde(skip)]
    failed: bool,
}
fn unavailable() -> NetError {
    NetError::new(
        "unavailable",
        "Could not save network activity on this device.",
    )
}
fn read(path: &Path) -> Result<Vec<u8>, NetError> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(unavailable()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(unavailable());
    }
    Ok(bytes)
}
/// Load only the bounded tail of an older/oversized audit, discarding its
/// first partial line. Counter files instead fail closed on oversize.
fn read_audit(path: &Path) -> Result<Vec<u8>, NetError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(unavailable()),
    };
    if !meta.is_file() {
        return Err(unavailable());
    }
    let mut file = std::fs::File::open(path).map_err(|_| unavailable())?;
    let tail = meta.len() > MAX_FILE;
    if tail {
        file.seek(SeekFrom::End(-(MAX_FILE as i64)))
            .map_err(|_| unavailable())?;
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if tail {
        let first = bytes
            .iter()
            .position(|b| *b == b'\n')
            .ok_or_else(unavailable)?;
        bytes.drain(..=first);
    }
    Ok(bytes)
}
fn write(path: &Path, bytes: &[u8]) -> Result<(), NetError> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().ok_or_else(unavailable)?)
        .map_err(|_| unavailable())?;
    temp.write_all(bytes).map_err(|_| unavailable())?;
    temp.as_file().sync_all().map_err(|_| unavailable())?;
    temp.persist(path).map_err(|_| unavailable())?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().ok_or_else(unavailable)?)
        .and_then(|f| f.sync_all())
        .map_err(|_| unavailable())?;
    Ok(())
}
impl State {
    pub(crate) fn open(dir: &Path) -> Result<Self, NetError> {
        std::fs::create_dir_all(dir).map_err(|_| unavailable())?;
        let bytes = read(&dir.join("network-counters.json"))?;
        let mut state: Self = if bytes.is_empty() {
            Self::default()
        } else {
            serde_json::from_slice(&bytes).map_err(|_| unavailable())?
        };
        state.dir = Some(dir.into());
        // Crash reservations remain charged. Unsent queued reservations count as
        // attempts too: never infer that a remote write did not happen.
        for (_, r) in std::mem::take(&mut state.reservations) {
            state.counters.entry(r.key).or_default().attempts.push(r.at);
        }
        state.persist()?;
        state.audit_rewrite(None)?;
        Ok(state)
    }
    fn tick(&mut self) -> u64 {
        self.clock = self.clock.max(now_ms());
        let day = self.clock / 86_400_000;
        for (key, counter) in &mut self.counters {
            counter
                .attempts
                .retain(|at| self.clock.saturating_sub(*at) < 3_600_000);
            if day > counter.day {
                counter.day = day;
                counter.bytes = self
                    .reservations
                    .values()
                    .filter(|r| &r.key == key)
                    .map(|r| r.bytes)
                    .sum();
            }
        }
        self.clock
    }
    fn persist(&mut self) -> Result<(), NetError> {
        if self.failed {
            return Err(unavailable());
        }
        if let Some(dir) = &self.dir {
            let bytes = serde_json::to_vec(self).map_err(|_| unavailable())?;
            if bytes.len() as u64 > MAX_FILE
                || write(&dir.join("network-counters.json"), &bytes).is_err()
            {
                self.failed = true;
                return Err(unavailable());
            }
        }
        Ok(())
    }
    pub(crate) fn capacity(&mut self, space: &str, plugin: &str) -> bool {
        self.tick();
        if self.failed {
            return false;
        }
        let key = serde_json::to_string(&(space, plugin)).expect("strings serialize");
        let Some(counter) = self.counters.get(&key) else {
            return true;
        };
        counter.attempts.len() + self.reservations.values().filter(|r| r.key == key).count() < 60
            && counter.bytes < DAILY
    }
    pub(crate) fn reserve(&mut self, id: &str, space: &str, plugin: &str) -> Result<(), NetError> {
        let at = self.tick();
        let key = serde_json::to_string(&(space, plugin)).expect("strings serialize");
        let counter = self.counters.entry(key.clone()).or_default();
        if counter.attempts.len() + self.reservations.values().filter(|r| r.key == key).count()
            >= 60
        {
            return Err(NetError::new(
                "rate-limited",
                "Too many web requests this hour.",
            ));
        }
        if counter.bytes >= DAILY {
            return Err(NetError::new(
                "quota",
                "The daily network limit was reached.",
            ));
        }
        self.reservations.insert(
            id.into(),
            Reservation {
                key,
                at,
                bytes: 0,
                started: false,
            },
        );
        self.persist()
    }
    pub(crate) fn start(&mut self, id: &str) -> Result<(), NetError> {
        let now = self.tick();
        let r = self.reservations.get_mut(id).ok_or_else(unavailable)?;
        let counter = self.counters.get_mut(&r.key).ok_or_else(unavailable)?;
        if counter.bytes.saturating_add(RESERVE) > DAILY {
            return Err(NetError::new(
                "quota",
                "The daily network limit was reached.",
            ));
        }
        counter.bytes += RESERVE;
        r.bytes = RESERVE;
        r.at = now;
        r.started = true;
        self.persist()
    }
    pub(crate) fn reserve_more(&mut self, id: &str, bytes: u64) -> Result<(), NetError> {
        self.tick();
        let r = self.reservations.get_mut(id).ok_or_else(unavailable)?;
        let c = self.counters.get_mut(&r.key).ok_or_else(unavailable)?;
        if c.bytes.saturating_add(bytes) > DAILY {
            return Err(NetError::new(
                "quota",
                "The daily network limit was reached.",
            ));
        }
        c.bytes += bytes;
        r.bytes += bytes;
        self.persist()
    }
    pub(crate) fn next_hop(&mut self, id: &str) -> Result<(), NetError> {
        let now = self.tick();
        let r = self.reservations.get(id).ok_or_else(unavailable)?;
        let counter = self.counters.get_mut(&r.key).ok_or_else(unavailable)?;
        if counter.attempts.len()
            + self
                .reservations
                .values()
                .filter(|v| v.key == r.key)
                .count()
            >= 60
        {
            return Err(NetError::new(
                "rate-limited",
                "Too many web requests this hour.",
            ));
        }
        counter.attempts.push(now);
        self.persist()
    }
    pub(crate) fn complete(&mut self, id: &str, bytes: u64) -> Result<(), NetError> {
        self.tick();
        if let Some(r) = self.reservations.remove(id) {
            let counter = self.counters.get_mut(&r.key).ok_or_else(unavailable)?;
            counter.bytes = counter
                .bytes
                .saturating_sub(r.bytes)
                .saturating_add(bytes.min(r.bytes));
            counter.attempts.push(r.at);
        }
        self.persist()
    }
    pub(crate) fn cancel(&mut self, id: &str) {
        if let Some(r) = self.reservations.remove(id) {
            if let Some(c) = self.counters.get_mut(&r.key) {
                c.bytes = c.bytes.saturating_sub(r.bytes);
            }
            let _ = self.persist();
        }
    }
    fn audit_rewrite(
        &mut self,
        change: Option<(&str, Option<Audit>)>,
    ) -> Result<Vec<Audit>, NetError> {
        let mut entries = if let Some(dir) = &self.dir {
            let bytes = read_audit(&dir.join("network-audit.jsonl"))?;
            bytes
                .split(|b| *b == b'\n')
                .filter(|b| !b.is_empty())
                .map(|line| {
                    if line.len() > 8192 {
                        return Err(unavailable());
                    }
                    serde_json::from_slice::<Audit>(line).map_err(|_| unavailable())
                })
                .collect::<Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        if let Some((plugin, entry)) = change {
            if let Some(entry) = entry {
                entries.push(entry);
            } else {
                entries.retain(|e| e.plugin != plugin);
            }
        }
        if entries.len() > 500 {
            entries.drain(..entries.len() - 500);
        }
        let mut lines = entries
            .iter()
            .map(|e| serde_json::to_string(e).expect("audit serializes") + "\n")
            .collect::<Vec<_>>();
        let mut size = lines.iter().map(String::len).sum::<usize>();
        while size > MAX_FILE as usize {
            size -= lines.remove(0).len();
            entries.remove(0);
        }
        if let Some(dir) = &self.dir {
            write(&dir.join("network-audit.jsonl"), lines.concat().as_bytes())?;
        }
        Ok(entries)
    }
    pub(crate) fn audit(&mut self, entry: Audit) -> Result<(), NetError> {
        self.audit_rewrite(Some(("", Some(entry)))).map(|_| ())
    }
    pub(crate) fn activity(&mut self, plugin: &str, clear: bool) -> Result<Vec<Audit>, NetError> {
        Ok(self
            .audit_rewrite(if clear { Some((plugin, None)) } else { None })?
            .into_iter()
            .filter(|e| e.plugin == plugin)
            .collect())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    pub at_ms: u64,
    pub space: String,
    pub plugin: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub method: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub host: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub ms: u64,
}
impl Audit {
    pub(crate) fn for_job(job: &Job, host: &str) -> Self {
        Self {
            at_ms: now_ms(),
            space: job.space.clone(),
            plugin: job.plugin.clone(),
            method: job.request.method.clone(),
            host: host.into(),
            status: None,
            code: None,
            bytes_in: 0,
            bytes_out: 0,
            ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_reservations_rollback_restart_and_clock_rollback() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = State::open(dir.path()).unwrap();
        for i in 0..60 {
            s.reserve(&i.to_string(), "space", "fixture").unwrap();
        }
        assert_eq!(
            s.reserve("overflow", "space", "fixture").unwrap_err().code,
            "rate-limited"
        );
        s.cancel("0");
        s.reserve("replacement", "space", "fixture").unwrap();
        drop(s);
        let mut s = State::open(dir.path()).unwrap();
        assert_eq!(
            s.reserve("restart", "space", "fixture").unwrap_err().code,
            "rate-limited"
        );
        s.clock = now_ms() + 86_400_000;
        s.tick();
        s.reserve("future", "space", "fixture").unwrap();
        assert!(s.clock > now_ms());
        let key = serde_json::to_string(&("space", "fixture")).unwrap();
        let counter = s.counters.get_mut(&key).unwrap();
        counter.bytes = DAILY - RESERVE + 1;
        assert_eq!(s.start("future").unwrap_err().code, "quota");
    }
    #[test]
    fn crash_keeps_byte_reservation_and_failure_is_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = State::open(dir.path()).unwrap();
        s.reserve("job", "s", "p").unwrap();
        s.start("job").unwrap();
        drop(s);
        let mut s = State::open(dir.path()).unwrap();
        assert_eq!(s.counters.values().next().unwrap().bytes, RESERVE);
        std::fs::remove_file(dir.path().join("network-counters.json")).unwrap();
        std::fs::create_dir(dir.path().join("network-counters.json")).unwrap();
        assert_eq!(s.reserve("next", "s", "p").unwrap_err().code, "unavailable");
        assert!(s.reserve("another", "s", "p").is_err());
    }
    #[test]
    fn oversized_audit_is_trimmed_on_load_with_bounded_reads() {
        let dir = tempfile::tempdir().unwrap();
        let entry = Audit {
            at_ms: 1,
            space: "synthetic".repeat(250),
            plugin: "fixture".into(),
            method: "GET".into(),
            host: "example.com".into(),
            status: Some(200),
            code: None,
            bytes_in: 1,
            bytes_out: 0,
            ms: 1,
        };
        let line = serde_json::to_string(&entry).unwrap() + "\n";
        std::fs::write(dir.path().join("network-audit.jsonl"), line.repeat(600)).unwrap();
        let mut state = State::open(dir.path()).unwrap();
        assert!(!state.activity("fixture", false).unwrap().is_empty());
        assert!(
            std::fs::metadata(dir.path().join("network-audit.jsonl"))
                .unwrap()
                .len()
                <= MAX_FILE
        );
        assert!(state.audit_rewrite(None).unwrap().len() <= 500);
    }
    #[test]
    fn audit_is_bounded_on_each_append_and_clear_is_plugin_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = State::open(dir.path()).unwrap();
        for i in 0..505 {
            s.audit(Audit {
                at_ms: i,
                space: "s".into(),
                plugin: if i % 2 == 0 { "a" } else { "b" }.into(),
                method: "GET".into(),
                host: "example.com".into(),
                status: Some(200),
                code: None,
                bytes_in: 1,
                bytes_out: 0,
                ms: 1,
            })
            .unwrap();
        }
        assert_eq!(s.audit_rewrite(None).unwrap().len(), 500);
        assert!(s.activity("a", true).unwrap().is_empty());
        assert!(!s.activity("b", false).unwrap().is_empty());
    }
}
