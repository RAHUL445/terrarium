//! Device-local replay protection and execution authority. Never registered for sync.
use crate::rules::{RuleInfo, Settings};
use crate::{Result, RuntimeError};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

fn problem() -> RuntimeError {
    RuntimeError::Plugin {
        code: "automation".into(),
        message: "Automation history is unavailable or full".into(),
    }
}
fn db<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|_| problem())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuleRunSummary {
    pub at_ms: u64,
    pub space: String,
    pub rule: String,
    pub event: String,
    pub plugin: String,
    pub operation: String,
    pub status: String,
    pub code: Option<String>,
    pub change_count: usize,
    #[serde(skip)]
    pub notify: bool,
}

pub(crate) struct Local {
    conn: Connection,
    audit: std::path::PathBuf,
}
impl Local {
    pub fn open(dir: &Path) -> Result<Self> {
        let path = dir.join("automation-local.sqlite");
        private_file(&path)?;
        let conn = db(Connection::open(path))?;
        db(conn.execute_batch("PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS claims (id TEXT PRIMARY KEY, rule TEXT NOT NULL, digest TEXT NOT NULL, at INTEGER NOT NULL, attempted INTEGER NOT NULL, outcome TEXT, history TEXT, published INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS grants (id TEXT PRIMARY KEY, digest TEXT NOT NULL, cutoff INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS authority (space TEXT PRIMARY KEY, settings TEXT NOT NULL, armed INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS clock (id INTEGER PRIMARY KEY, at INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS notices (rule TEXT PRIMARY KEY, at INTEGER NOT NULL);"))?;
        Ok(Self {
            conn,
            audit: dir.join("automation-audit.jsonl"),
        })
    }
    pub fn observe(&self, space: &str, settings: &Settings) -> Result<bool> {
        let signature = serde_json::to_string(settings)?;
        db(self.conn.execute(
            "UPDATE authority SET armed=0, settings=?2 WHERE space=?1 AND (settings<>?2 OR ?3)",
            params![space, signature, settings.paused],
        ))?;
        db(self
            .conn
            .query_row(
                "SELECT armed FROM authority WHERE space=?1 AND settings=?2",
                params![space, signature],
                |r| r.get::<_, bool>(0),
            )
            .optional())
        .map(|v| v.unwrap_or(false))
    }
    pub fn arm(&self, space: &str, settings: &Settings, device: &str) -> Result<bool> {
        let armed = !settings.paused && settings.primary_device.as_deref() == Some(device);
        db(self.conn.execute("INSERT INTO authority VALUES (?1,?2,?3) ON CONFLICT(space) DO UPDATE SET settings=excluded.settings,armed=excluded.armed", params![space, serde_json::to_string(settings)?,armed]))?;
        Ok(armed)
    }
    pub fn grant(&self, rule: &RuleInfo, now: u64) -> Result<()> {
        db(self.conn.execute("INSERT INTO grants VALUES (?1,?2,?3) ON CONFLICT(id) DO UPDATE SET digest=excluded.digest,cutoff=excluded.cutoff", params![rule.id,crate::rules::digest(rule),now.min(i64::MAX as u64) as i64]))?;
        Ok(())
    }
    pub fn revoke(&self, id: &str) -> Result<()> {
        db(self.conn.execute("DELETE FROM grants WHERE id=?1", [id]))?;
        Ok(())
    }
    pub fn granted(&self, rule: &RuleInfo, emitted: u64) -> Result<bool> {
        let cutoff = db(self
            .conn
            .query_row(
                "SELECT cutoff FROM grants WHERE id=?1 AND digest=?2",
                params![rule.id, crate::rules::digest(rule)],
                |r| r.get::<_, i64>(0),
            )
            .optional())?;
        Ok(cutoff.is_some_and(|c| emitted >= c as u64))
    }
    pub fn clock(&self, now: u64) -> Result<u64> {
        db(self.conn.execute(
            "INSERT INTO clock VALUES (1,?1) ON CONFLICT(id) DO UPDATE SET at=max(at,excluded.at)",
            [now.min(i64::MAX as u64) as i64],
        ))?;
        db(self
            .conn
            .query_row("SELECT at FROM clock WHERE id=1", [], |r| {
                r.get::<_, i64>(0)
            }))
        .map(|at| at as u64)
    }
    pub fn has_claim(&self, id: &str) -> Result<bool> {
        db(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM claims WHERE id=?1)",
            [id],
            |r| r.get(0),
        ))
    }
    pub fn limited(&self, rule: &str, now: u64) -> Result<bool> {
        let (rule_count, all): (i64, i64) = db(self.conn.query_row(
            "SELECT coalesce(sum(rule=?1),0),count(*) FROM claims WHERE attempted=1 AND at>=?2",
            params![
                rule,
                now.saturating_sub(3_600_000).min(i64::MAX as u64) as i64
            ],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        Ok(rule_count >= crate::rules::MAX_RUNS_PER_RULE_PER_HOUR as i64 || all >= 600)
    }
    /// Durable insert precedes invocation. No eviction of replay protection.
    pub fn claim(
        &self,
        id: &str,
        rule: &RuleInfo,
        now: u64,
        attempted: bool,
        history: &serde_json::Value,
        summary: &RuleRunSummary,
    ) -> Result<()> {
        let count: i64 = db(self
            .conn
            .query_row("SELECT count(*) FROM claims", [], |r| r.get(0)))?;
        if count >= 100_000 {
            return Err(RuntimeError::Plugin {
                code: "automation_full".into(),
                message: "Automation history is full".into(),
            });
        }
        db(self.conn.execute("INSERT INTO claims (id,rule,digest,at,attempted,history,outcome) VALUES (?1,?2,?3,?4,?5,?6,?7)", params![id,rule.id,crate::rules::digest(rule),now.min(i64::MAX as u64) as i64,attempted,serde_json::to_string(history)?,serde_json::to_string(summary)?]))?;
        Ok(())
    }
    pub fn finish(
        &self,
        id: &str,
        summary: &RuleRunSummary,
        history: &serde_json::Value,
    ) -> Result<()> {
        db(self.conn.execute(
            "UPDATE claims SET outcome=?2,history=?3 WHERE id=?1",
            params![
                id,
                serde_json::to_string(summary)?,
                serde_json::to_string(history)?
            ],
        ))?;
        Ok(())
    }
    pub fn unpublished(&self) -> Result<Vec<(String, RuleRunSummary, serde_json::Value)>> {
        let mut stmt = db(self
            .conn
            .prepare("SELECT id,outcome,history FROM claims WHERE published=0 ORDER BY at,id"))?;
        let rows = db(stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        }))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, s, h) = db(row)?;
            out.push((id, serde_json::from_str(&s)?, serde_json::from_str(&h)?));
        }
        Ok(out)
    }
    pub fn published(&self, id: &str) -> Result<()> {
        db(self
            .conn
            .execute("UPDATE claims SET published=1 WHERE id=?1", [id]))?;
        Ok(())
    }
    pub fn notify(&self, rule: &str, now: u64) -> Result<bool> {
        let last = db(self
            .conn
            .query_row("SELECT at FROM notices WHERE rule=?1", [rule], |r| {
                r.get::<_, i64>(0)
            })
            .optional())?;
        if last.is_some_and(|last| now.saturating_sub(last as u64) < 3_600_000) {
            return Ok(false);
        }
        db(self.conn.execute(
            "INSERT INTO notices VALUES (?1,?2) ON CONFLICT(rule) DO UPDATE SET at=excluded.at",
            params![rule, now.min(i64::MAX as u64) as i64],
        ))?;
        Ok(true)
    }
    pub fn audit_ready(&self) -> Result<()> {
        private_file(&self.audit).map(|_| ())
    }
    pub fn audit(&self, summary: &RuleRunSummary) -> Result<()> {
        let mut file = private_file(&self.audit)?;
        if file.metadata().map_err(|_| problem())?.len() > 512 * 1024 {
            let text = std::fs::read_to_string(&self.audit).map_err(|_| problem())?;
            let mut lines: Vec<_> = text.lines().rev().take(199).collect();
            lines.reverse();
            let mut tmp =
                tempfile::NamedTempFile::new_in(self.audit.parent().expect("data directory"))
                    .map_err(|_| problem())?;
            for line in lines {
                writeln!(tmp, "{line}").map_err(|_| problem())?;
            }
            tmp.as_file().sync_all().map_err(|_| problem())?;
            tmp.persist(&self.audit).map_err(|_| problem())?;
            file = private_file(&self.audit)?;
        }
        writeln!(file, "{}", serde_json::to_string(summary)?).map_err(|_| problem())?;
        file.sync_all().map_err(|_| problem())
    }
}
fn private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|_| problem())
}
