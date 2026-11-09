//! One plugin's database in one space: host-mediated writes, the change log,
//! and deterministic merging of local and remote changes.
//!
//! Every write, local or remote, goes through [`apply_change`]. Local writes
//! are validated by [`Writer`] first; remote changes arrive in [`Batch`]es.
//! Delivery is causal per sender because exports follow local log order, so
//! a device always receives a change's ancestors before the change itself.

use std::collections::BTreeMap;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::change::{Batch, Change, Op, VersionVector};
use crate::error::{Error, Result};
use crate::hlc::{Clock, DeviceId, Hlc, TimeSource};
use crate::read_policy::{ReadPolicy, MAX_OUTPUT_BYTES, MAX_ROWS, MAX_SQL_BYTES};
use crate::schema::{
    DeletePolicy, MergeRule, PluginSchema, TableDef, TableKind, DELETED_COLUMN, ID_COLUMN,
};
use crate::value::{from_sql, to_sql};

const META_DDL: &str = "
CREATE TABLE IF NOT EXISTS _pp_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS _pp_changes(
    lseq INTEGER PRIMARY KEY AUTOINCREMENT,
    hlc TEXT NOT NULL UNIQUE,
    device TEXT NOT NULL,
    user_id TEXT NOT NULL,
    tbl TEXT NOT NULL,
    row_id TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    op TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS _pp_changes_row ON _pp_changes(tbl, row_id);
CREATE TABLE IF NOT EXISTS _pp_cells(tbl TEXT, row_id TEXT, col TEXT, hlc TEXT NOT NULL, PRIMARY KEY(tbl, row_id, col));
CREATE TABLE IF NOT EXISTS _pp_rows(tbl TEXT, row_id TEXT, state TEXT NOT NULL, deleted_at_ms INTEGER, PRIMARY KEY(tbl, row_id));
CREATE TABLE IF NOT EXISTS _pp_conflicts(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    tbl TEXT NOT NULL, row_id TEXT NOT NULL, col TEXT NOT NULL,
    kept_value TEXT NOT NULL, kept_hlc TEXT NOT NULL,
    lost_value TEXT NOT NULL, lost_hlc TEXT NOT NULL,
    recorded_ms INTEGER NOT NULL,
    UNIQUE(tbl, row_id, col, lost_hlc));
CREATE TABLE IF NOT EXISTS _pp_notices(tbl TEXT, row_id TEXT, kind TEXT, hlc TEXT, PRIMARY KEY(tbl, row_id, kind, hlc));
CREATE TABLE IF NOT EXISTS _pp_pending(seq INTEGER PRIMARY KEY AUTOINCREMENT, hlc TEXT NOT NULL UNIQUE, change TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS _pp_seen(device TEXT PRIMARY KEY, hlc TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS _pp_peer_acks(peer TEXT, origin TEXT, hlc TEXT NOT NULL, PRIMARY KEY(peer, origin));
";

#[derive(Clone, Debug)]
pub struct StoreConfig {
    pub space: String,
    pub plugin: String,
    pub device: DeviceId,
    pub user: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RowState {
    Live,
    Trashed,
    /// Removed by a `delete-wins` delete or purged from trash; terminal.
    Gone,
}

impl RowState {
    fn as_str(self) -> &'static str {
        match self {
            RowState::Live => "live",
            RowState::Trashed => "trashed",
            RowState::Gone => "gone",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "live" => Ok(RowState::Live),
            "trashed" => Ok(RowState::Trashed),
            "gone" => Ok(RowState::Gone),
            other => Err(Error::Corrupt(format!("row state `{other}`"))),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ImportReport {
    pub applied: usize,
    pub duplicates: usize,
    pub pending: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChangeRecord {
    pub lseq: i64,
    pub change: Change,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConflictRecord {
    pub table: String,
    pub row: String,
    pub column: String,
    pub kept_value: Value,
    pub kept_hlc: Hlc,
    pub lost_value: Value,
    pub lost_hlc: Hlc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NoticeRecord {
    pub table: String,
    pub row: String,
    /// `edited_in_trash` or `restored_by_edit`.
    pub kind: String,
    pub hlc: Hlc,
}

enum Outcome {
    Applied,
    Duplicate,
    Pending,
}

/// The plugin stores of one space, as seen by sync.
pub trait StoreSet {
    fn plugin_ids(&self) -> Vec<String>;
    fn store_mut(&mut self, plugin: &str) -> Option<&mut Store>;
}

impl StoreSet for BTreeMap<String, Store> {
    fn plugin_ids(&self) -> Vec<String> {
        self.keys().cloned().collect()
    }

    fn store_mut(&mut self, plugin: &str) -> Option<&mut Store> {
        self.get_mut(plugin)
    }
}

pub struct Store {
    conn: Connection,
    cfg: StoreConfig,
    schema: PluginSchema,
    clock: Clock,
    reads: ReadPolicy,
}

impl Store {
    /// Opens (or creates) the database and brings it to `schema`, running
    /// additive migrations and replaying changes buffered for this version.
    pub fn open(
        path: impl AsRef<Path>,
        cfg: StoreConfig,
        schema: PluginSchema,
        time: Box<dyn TimeSource>,
    ) -> Result<Self> {
        schema.validate()?;
        let mut conn = Connection::open(path)?;
        conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
        conn.execute_batch(META_DDL)?;

        let tx = conn.transaction()?;
        let stored: Option<String> = tx
            .query_row("SELECT value FROM _pp_meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .optional()?;
        let from = match stored {
            None => None,
            Some(json) => Some(serde_json::from_str::<PluginSchema>(&json)?),
        };
        match &from {
            Some(old) if schema.version < old.version => {
                return Err(Error::Downgrade {
                    stored: old.version,
                    requested: schema.version,
                })
            }
            Some(old) if schema.version == old.version => {
                if &schema != old {
                    return Err(Error::Migration(
                        "schema changed without a version bump".into(),
                    ));
                }
            }
            Some(old) => old.check_migration(&schema)?,
            None => {}
        }
        if from.as_ref() != Some(&schema) {
            for statement in schema.ddl_from(from.as_ref()) {
                tx.execute(&statement, [])?;
            }
            tx.execute(
                "INSERT INTO _pp_meta(key, value) VALUES('schema', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [serde_json::to_string(&schema)?],
            )?;
        }
        tx.commit()?;

        let mut clock = Clock::new(cfg.device.clone(), time);
        let latest: Option<String> =
            conn.query_row("SELECT MAX(hlc) FROM _pp_changes", [], |r| r.get(0))?;
        if let Some(latest) = latest {
            clock.observe(&latest.parse()?);
        }
        let reads = ReadPolicy::install(&mut conn, &schema)?;
        let mut store = Store {
            reads,
            conn,
            cfg,
            schema,
            clock,
        };
        store.replay_pending()?;
        Ok(store)
    }

    /// The schema stored in an existing database, without opening it for
    /// writes. Lets a host keep a newer schema when an older plugin version
    /// is rolled back to (migrations are additive, so older code still works).
    pub fn stored_schema(path: impl AsRef<Path>) -> Result<Option<PluginSchema>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let json: Option<String> = conn
            .query_row("SELECT value FROM _pp_meta WHERE key = 'schema'", [], |r| {
                r.get(0)
            })
            .optional()
            .or_else(|e| match e {
                rusqlite::Error::SqliteFailure(..) => Ok(None),
                e => Err(e),
            })?;
        Ok(match json {
            Some(json) => Some(serde_json::from_str(&json)?),
            None => None,
        })
    }

    pub fn config(&self) -> &StoreConfig {
        &self.cfg
    }

    pub fn schema(&self) -> &PluginSchema {
        &self.schema
    }

    /// Runs local writes in one transaction; all or nothing.
    /// Inside [`Store::begin`] it becomes a savepoint, so a failed call
    /// leaves no partial writes even if the caller carries on.
    pub fn write<T>(&mut self, f: impl FnOnce(&mut Writer<'_>) -> Result<T>) -> Result<T> {
        let sp = self.conn.savepoint()?;
        let mut writer = Writer {
            conn: &sp,
            schema: &self.schema,
            cfg: &self.cfg,
            clock: &mut self.clock,
        };
        let out = f(&mut writer)?;
        sp.commit()?;
        Ok(out)
    }

    /// Starts a transaction spanning several [`Store::write`] calls, e.g. one
    /// plugin operation. End it with [`Store::commit`] or [`Store::rollback`].
    pub fn begin(&mut self) -> Result<()> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<()> {
        if !self.conn.is_autocommit() {
            self.conn.execute_batch("ROLLBACK")?;
        }
        Ok(())
    }

    /// Checks that `sql` compiles as a read-only query over plugin tables,
    /// returning how many `?N` parameters it takes.
    pub fn check_query(&self, sql: &str) -> Result<usize> {
        if sql.len() > MAX_SQL_BYTES {
            return Err(Error::QueryLimit("SQL length"));
        }
        let _guard = self.reads.enter(&self.conn, true, false)?;
        Ok(self.prepare_read(sql)?.parameter_count())
    }

    fn prepare_read(&self, sql: &str) -> Result<rusqlite::Statement<'_>> {
        let stmt = self.conn.prepare(sql).map_err(|e| {
            if e.sqlite_error_code() == Some(rusqlite::ErrorCode::AuthorizationForStatementDenied) {
                Error::ReadOnlyQuery
            } else {
                Error::Sqlite(e)
            }
        })?;
        if !stmt.readonly() {
            return Err(Error::ReadOnlyQuery);
        }
        Ok(stmt)
    }

    /// Read-only SQL over this plugin's tables. Rows include `id` and
    /// `_deleted` (the delete timestamp for rows in trash, else null).
    pub fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Map<String, Value>>> {
        self.query_read(sql, params, false, false)
    }

    /// Plugin-facing reads: bounded SQL, parameters, SQLite work/values,
    /// returned rows and encoded JSON. Fails rather than returning partial data.
    pub fn query_bounded(&self, sql: &str, params: &[Value]) -> Result<Vec<Map<String, Value>>> {
        self.bounded_query(sql, params, false)
    }

    /// Host-only larger value envelope for permission-checked audio work.
    pub fn query_bounded_media(
        &self,
        sql: &str,
        params: &[Value],
    ) -> Result<Vec<Map<String, Value>>> {
        self.bounded_query(sql, params, true)
    }

    fn bounded_query(
        &self,
        sql: &str,
        params: &[Value],
        media: bool,
    ) -> Result<Vec<Map<String, Value>>> {
        if sql.len() > MAX_SQL_BYTES || params.len() > 128 {
            return Err(Error::QueryLimit("SQL or parameter count"));
        }
        let mut budget = JsonBudget(1024 * 1024);
        if serde_json::to_writer(&mut budget, params).is_err() {
            return Err(Error::QueryLimit("parameters"));
        }
        self.query_read(sql, params, true, media)
    }

    fn query_read(
        &self,
        sql: &str,
        params: &[Value],
        bounded: bool,
        media: bool,
    ) -> Result<Vec<Map<String, Value>>> {
        let _guard = self.reads.enter(&self.conn, bounded, media)?;
        let mut stmt = self.prepare_read(sql)?;
        let names: Vec<String> = stmt.column_names().map(String::from).collect();
        let args: Vec<_> = params.iter().map(to_sql).collect();
        let mut rows = stmt.query(rusqlite::params_from_iter(args))?;
        let mut out = Vec::new();
        let mut budget = JsonBudget(if media {
            16 * 1024 * 1024 - 2
        } else {
            MAX_OUTPUT_BYTES - 2
        });
        while let Some(row) = rows
            .next()
            .map_err(|e| match (bounded, e.sqlite_error_code()) {
                (true, Some(rusqlite::ErrorCode::OperationInterrupted)) => {
                    Error::QueryLimit("SQLite work budget")
                }
                (true, Some(rusqlite::ErrorCode::TooBig)) => Error::QueryLimit("SQLite value size"),
                _ => Error::Sqlite(e),
            })?
        {
            if bounded && out.len() == MAX_ROWS {
                return Err(Error::QueryLimit("row count"));
            }
            let mut map = Map::new();
            for (i, name) in names.iter().enumerate() {
                map.insert(name.clone(), from_sql(row.get_ref(i)?));
            }
            if bounded
                && ((!out.is_empty() && std::io::Write::write_all(&mut budget, b",").is_err())
                    || serde_json::to_writer(&mut budget, &map).is_err())
            {
                return Err(Error::QueryLimit("result size"));
            }
            out.push(map);
        }
        Ok(out)
    }

    pub fn row_state(&self, table: &str, row: &str) -> Result<Option<RowState>> {
        row_state(&self.conn, table, row)
    }

    /// Every table's rows ordered by id, for inspection and convergence checks.
    pub fn dump(&self) -> Result<BTreeMap<String, Vec<Map<String, Value>>>> {
        let mut out = BTreeMap::new();
        for table in &self.schema.tables {
            let rows = self.query(
                &format!("SELECT * FROM \"{}\" ORDER BY \"{ID_COLUMN}\"", table.name),
                &[],
            )?;
            out.insert(table.name.clone(), rows);
        }
        Ok(out)
    }

    /// Highest timestamp applied from each origin device.
    pub fn seen(&self) -> Result<VersionVector> {
        let mut stmt = self.conn.prepare("SELECT device, hlc FROM _pp_seen")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = VersionVector::new();
        for row in rows {
            let (device, hlc) = row?;
            out.insert(DeviceId(device), hlc.parse()?);
        }
        Ok(out)
    }

    /// Changes in local log order after `after_lseq`, plus the new cursor.
    pub fn export_since(&self, after_lseq: i64) -> Result<(Batch, i64)> {
        let mut cursor = after_lseq;
        let mut changes = Vec::new();
        for record in self.change_records("WHERE lseq > ?1 ORDER BY lseq", params![after_lseq])? {
            cursor = record.lseq;
            changes.push(record.change);
        }
        let batch = Batch {
            plugin: self.cfg.plugin.clone(),
            sender: self.cfg.device.clone(),
            sender_seen: self.seen()?,
            changes,
        };
        Ok((batch, cursor))
    }

    pub fn import(&mut self, batch: &Batch) -> Result<ImportReport> {
        if batch.plugin != self.cfg.plugin {
            return Err(Error::Corrupt(format!(
                "batch for plugin `{}`",
                batch.plugin
            )));
        }
        let tx = self.conn.transaction()?;
        let mut report = ImportReport::default();
        for change in &batch.changes {
            self.clock.observe(&change.hlc);
            match apply_change(&tx, &self.schema, change, self.clock.now_ms())? {
                Outcome::Applied => report.applied += 1,
                Outcome::Duplicate => report.duplicates += 1,
                Outcome::Pending => report.pending += 1,
            }
        }
        for (origin, hlc) in &batch.sender_seen {
            tx.execute(
                "INSERT INTO _pp_peer_acks(peer, origin, hlc) VALUES(?1, ?2, ?3)
                 ON CONFLICT(peer, origin) DO UPDATE SET hlc = excluded.hlc WHERE excluded.hlc > _pp_peer_acks.hlc",
                params![batch.sender.0, origin.0, hlc.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(report)
    }

    /// Removes trashed rows older than `retention_ms` once every peer in
    /// `peers` has confirmed it received the delete. Purging is local; each
    /// device reaches the same result once the same acknowledgements arrive.
    pub fn purge(&mut self, peers: &[DeviceId], retention_ms: u64) -> Result<usize> {
        let cutoff = self.clock.now_ms().saturating_sub(retention_ms) as i64;
        let tx = self.conn.transaction()?;
        let mut purged = 0;
        for table in &self.schema.tables {
            if table.delete_policy == DeletePolicy::DeleteWins {
                continue;
            }
            let candidates: Vec<String> = {
                let mut stmt = tx.prepare(
                    "SELECT row_id FROM _pp_rows WHERE tbl = ?1 AND state = 'trashed' AND deleted_at_ms <= ?2",
                )?;
                let rows = stmt.query_map(params![table.name, cutoff], |r| r.get(0))?;
                rows.collect::<std::result::Result<_, _>>()?
            };
            for row in candidates {
                let deleted: String = tx.query_row(
                    &format!(
                        "SELECT \"{DELETED_COLUMN}\" FROM \"{}\" WHERE \"{ID_COLUMN}\" = ?1",
                        table.name
                    ),
                    [&row],
                    |r| r.get(0),
                )?;
                let delete: Hlc = deleted.parse()?;
                let mut acknowledged = true;
                for peer in peers.iter().filter(|p| **p != self.cfg.device) {
                    let ack: Option<String> = tx
                        .query_row(
                            "SELECT hlc FROM _pp_peer_acks WHERE peer = ?1 AND origin = ?2",
                            params![peer.0, delete.device.0],
                            |r| r.get(0),
                        )
                        .optional()?;
                    if ack.is_none_or(|a| a < deleted) {
                        acknowledged = false;
                        break;
                    }
                }
                if acknowledged {
                    remove_row(&tx, &table.name, &row)?;
                    purged += 1;
                }
            }
        }
        tx.commit()?;
        Ok(purged)
    }

    pub fn changes(&self, limit: usize) -> Result<Vec<ChangeRecord>> {
        self.change_records("ORDER BY lseq DESC LIMIT ?1", params![limit as i64])
    }

    pub fn conflicts(&self) -> Result<Vec<ConflictRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT tbl, row_id, col, kept_value, kept_hlc, lost_value, lost_hlc FROM _pp_conflicts ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (table, row, column, kept_value, kept_hlc, lost_value, lost_hlc) = row?;
            out.push(ConflictRecord {
                table,
                row,
                column,
                kept_value: serde_json::from_str(&kept_value)?,
                kept_hlc: kept_hlc.parse()?,
                lost_value: serde_json::from_str(&lost_value)?,
                lost_hlc: lost_hlc.parse()?,
            });
        }
        Ok(out)
    }

    /// Drops conflict history older than `retention_ms`.
    pub fn prune_conflicts(&mut self, retention_ms: u64) -> Result<usize> {
        let cutoff = self.clock.now_ms().saturating_sub(retention_ms) as i64;
        Ok(self
            .conn
            .execute("DELETE FROM _pp_conflicts WHERE recorded_ms < ?1", [cutoff])?)
    }

    pub fn notices(&self) -> Result<Vec<NoticeRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tbl, row_id, kind, hlc FROM _pp_notices ORDER BY hlc")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (table, row, kind, hlc) = row?;
            out.push(NoticeRecord {
                table,
                row,
                kind,
                hlc: hlc.parse()?,
            });
        }
        Ok(out)
    }

    fn change_records(&self, tail: &str, args: impl rusqlite::Params) -> Result<Vec<ChangeRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT lseq, hlc, user_id, tbl, row_id, schema_version, op FROM _pp_changes {tail}"
        ))?;
        let rows = stmt.query_map(args, |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, u32>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (lseq, hlc, user, table, row, schema_version, op) = row?;
            out.push(ChangeRecord {
                lseq,
                change: Change {
                    hlc: hlc.parse()?,
                    user,
                    table,
                    row,
                    schema_version,
                    op: serde_json::from_str(&op)?,
                },
            });
        }
        Ok(out)
    }

    fn replay_pending(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        let pending: Vec<(i64, String)> = {
            let mut stmt = tx.prepare("SELECT seq, change FROM _pp_pending ORDER BY seq")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for (seq, json) in pending {
            let change: Change = serde_json::from_str(&json)?;
            if change.schema_version > self.schema.version {
                continue;
            }
            tx.execute("DELETE FROM _pp_pending WHERE seq = ?1", [seq])?;
            apply_change(&tx, &self.schema, &change, self.clock.now_ms())?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Local writes inside one transaction. Each call validates against the
/// schema, then emits changes through the same path remote changes take.
pub struct Writer<'a> {
    conn: &'a Connection,
    schema: &'a PluginSchema,
    cfg: &'a StoreConfig,
    clock: &'a mut Clock,
}

impl Writer<'_> {
    /// Inserts a row with a device-generated UUIDv7 id and returns the id.
    pub fn insert(&mut self, table: &str, values: &Map<String, Value>) -> Result<String> {
        let def = self.schema.table(table)?;
        for (name, value) in values {
            def.column(name)?.check_value(table, value)?;
        }
        let row = uuid::Uuid::now_v7().to_string();
        self.emit(table, &row, Op::Create)?;
        for (name, value) in values {
            match def.column(name)?.rule {
                MergeRule::Counter => {
                    if value.as_f64() != Some(0.0) {
                        self.emit(
                            table,
                            &row,
                            Op::Increment {
                                column: name.clone(),
                                delta: value.clone(),
                            },
                        )?;
                    }
                }
                MergeRule::Text => self.text_edit(table, &row, name, value)?,
                MergeRule::Lww => self.emit(
                    table,
                    &row,
                    Op::Set {
                        column: name.clone(),
                        value: value.clone(),
                        base: None,
                    },
                )?,
            }
        }
        Ok(row)
    }

    /// Creates the row `row` if needed, then sets `values` like [`Writer::update`].
    /// For rows whose id is chosen by the caller (e.g. one row per plugin id):
    /// devices creating the same row offline end up with one row.
    pub fn upsert(&mut self, table: &str, row: &str, values: &Map<String, Value>) -> Result<()> {
        let def = self.schema.table(table)?;
        if def.kind == TableKind::Append {
            return Err(Error::AppendOnly(table.to_string()));
        }
        match row_state(self.conn, table, row)? {
            None => {
                for (name, value) in values {
                    def.column(name)?.check_value(table, value)?;
                }
                self.emit(table, row, Op::Create)?;
            }
            Some(RowState::Trashed) => self.emit(table, row, Op::Restore)?,
            _ => {}
        }
        self.update(table, row, values)
    }

    /// Last-writer-wins update of non-counter columns on a live row.
    pub fn update(&mut self, table: &str, row: &str, values: &Map<String, Value>) -> Result<()> {
        let def = self.schema.table(table)?;
        if def.kind == TableKind::Append {
            return Err(Error::AppendOnly(table.to_string()));
        }
        self.require_live(def, row)?;
        for (name, value) in values {
            let column = def.column(name)?;
            if column.rule == MergeRule::Counter {
                return Err(Error::IsCounter {
                    table: table.to_string(),
                    column: name.clone(),
                });
            }
            column.check_value(table, value)?;
        }
        for (name, value) in values {
            if def.column(name)?.rule == MergeRule::Text {
                self.text_edit(table, row, name, value)?;
                continue;
            }
            let base = cell_hlc(self.conn, table, row, name)?;
            self.emit(
                table,
                row,
                Op::Set {
                    column: name.clone(),
                    value: value.clone(),
                    base,
                },
            )?;
        }
        Ok(())
    }

    /// Adds `delta` to a counter column; increments from all devices add up.
    pub fn increment(&mut self, table: &str, row: &str, column: &str, delta: Value) -> Result<()> {
        let def = self.schema.table(table)?;
        let col = def.column(column)?;
        if col.rule != MergeRule::Counter {
            return Err(Error::NotCounter {
                table: table.to_string(),
                column: column.to_string(),
            });
        }
        col.check_value(table, &delta)?;
        self.require_live(def, row)?;
        self.emit(
            table,
            row,
            Op::Increment {
                column: column.to_string(),
                delta,
            },
        )
    }

    pub fn delete(&mut self, table: &str, row: &str) -> Result<()> {
        let def = self.schema.table(table)?;
        if def.kind == TableKind::Append {
            return Err(Error::AppendOnly(table.to_string()));
        }
        self.require_live(def, row)?;
        let seen = row_seen(self.conn, table, row)?;
        self.emit(table, row, Op::Delete { seen })
    }

    /// Moves a row out of trash.
    pub fn restore(&mut self, table: &str, row: &str) -> Result<()> {
        let def = self.schema.table(table)?;
        if def.delete_policy == DeletePolicy::DeleteWins {
            return Err(Error::Policy {
                table: table.to_string(),
                reason: "delete-wins rows cannot be restored",
            });
        }
        match row_state(self.conn, table, row)? {
            Some(RowState::Trashed) => self.emit(table, row, Op::Restore),
            _ => Err(Error::NotFound {
                table: table.to_string(),
                row: row.to_string(),
            }),
        }
    }

    /// Emits the CRDT update that makes a `text` cell equal `value` (null
    /// counts as empty), merging with any concurrent edits elsewhere.
    fn text_edit(&mut self, table: &str, row: &str, column: &str, value: &Value) -> Result<()> {
        let new = value.as_str().unwrap_or("");
        let def = self.schema.table(table)?.column(column)?;
        let state = read_blob(self.conn, table, row, &def.crdt_column())?;
        if let Some(update) = crate::text::edit(state.as_deref(), self.clock.device(), new)? {
            self.emit(
                table,
                row,
                Op::TextEdit {
                    column: column.to_string(),
                    update: B64.encode(update),
                },
            )?;
        }
        Ok(())
    }

    fn require_live(&self, def: &TableDef, row: &str) -> Result<()> {
        match row_state(self.conn, &def.name, row)? {
            Some(RowState::Live) => Ok(()),
            Some(RowState::Trashed) => Err(Error::InTrash {
                table: def.name.clone(),
                row: row.to_string(),
            }),
            _ => Err(Error::NotFound {
                table: def.name.clone(),
                row: row.to_string(),
            }),
        }
    }

    fn emit(&mut self, table: &str, row: &str, op: Op) -> Result<()> {
        let change = Change {
            hlc: self.clock.tick(),
            user: self.cfg.user.clone(),
            table: table.to_string(),
            row: row.to_string(),
            schema_version: self.schema.version,
            op,
        };
        apply_change(self.conn, self.schema, &change, self.clock.now_ms())?;
        Ok(())
    }
}

fn apply_change(
    conn: &Connection,
    schema: &PluginSchema,
    change: &Change,
    now_ms: u64,
) -> Result<Outcome> {
    let hlc = change.hlc.to_string();
    let known: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM _pp_changes WHERE hlc = ?1) OR EXISTS(SELECT 1 FROM _pp_pending WHERE hlc = ?1)",
        [&hlc],
        |r| r.get(0),
    )?;
    if known {
        return Ok(Outcome::Duplicate);
    }
    if change.schema_version > schema.version {
        conn.execute(
            "INSERT INTO _pp_pending(hlc, change) VALUES(?1, ?2)",
            params![hlc, serde_json::to_string(change)?],
        )?;
        return Ok(Outcome::Pending);
    }
    let table = schema.table(&change.table)?;
    validate_remote(table, &change.op)?;

    conn.execute(
        "INSERT INTO _pp_changes(hlc, device, user_id, tbl, row_id, schema_version, op) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            hlc,
            change.hlc.device.0,
            change.user,
            change.table,
            change.row,
            change.schema_version,
            serde_json::to_string(&change.op)?
        ],
    )?;
    conn.execute(
        "INSERT INTO _pp_seen(device, hlc) VALUES(?1, ?2)
         ON CONFLICT(device) DO UPDATE SET hlc = excluded.hlc WHERE excluded.hlc > _pp_seen.hlc",
        params![change.hlc.device.0, hlc],
    )?;

    let row = change.row.as_str();
    if row_state(conn, &table.name, row)? == Some(RowState::Gone) {
        return Ok(Outcome::Applied);
    }
    ensure_row(conn, &table.name, row)?;
    match &change.op {
        Op::Create => {}
        Op::Set {
            column,
            value,
            base,
        } => {
            let current = cell_hlc(conn, &table.name, row, column)?;
            let current_value = read_cell(conn, &table.name, row, column)?;
            let concurrent = current.is_some() && base.as_ref() != current.as_ref();
            match current {
                Some(cur) if cur > change.hlc => {
                    if concurrent {
                        record_conflict(
                            conn,
                            &table.name,
                            row,
                            column,
                            (&current_value, &cur),
                            (value, &change.hlc),
                            now_ms,
                        )?;
                    }
                }
                _ => {
                    if let (true, Some(cur)) = (concurrent, &current) {
                        record_conflict(
                            conn,
                            &table.name,
                            row,
                            column,
                            (value, &change.hlc),
                            (&current_value, cur),
                            now_ms,
                        )?;
                    }
                    conn.execute(
                        &format!(
                            "UPDATE \"{}\" SET \"{column}\" = ?1 WHERE \"{ID_COLUMN}\" = ?2",
                            table.name
                        ),
                        params![to_sql(value), row],
                    )?;
                    conn.execute(
                        "INSERT INTO _pp_cells(tbl, row_id, col, hlc) VALUES(?1, ?2, ?3, ?4)
                         ON CONFLICT(tbl, row_id, col) DO UPDATE SET hlc = excluded.hlc",
                        params![table.name, row, column, hlc],
                    )?;
                }
            }
            recompute_if_trashed(conn, table, row)?;
        }
        Op::Increment { column, delta } => {
            conn.execute(
                &format!(
                    "UPDATE \"{}\" SET \"{column}\" = COALESCE(\"{column}\", 0) + ?1 WHERE \"{ID_COLUMN}\" = ?2",
                    table.name
                ),
                params![to_sql(delta), row],
            )?;
            recompute_if_trashed(conn, table, row)?;
        }
        Op::TextEdit { column, update } => {
            let crdt = table.column(column)?.crdt_column();
            let update = B64
                .decode(update)
                .map_err(|_| Error::Corrupt("text update is not base64".into()))?;
            let state = read_blob(conn, &table.name, row, &crdt)?;
            let (state, text) = crate::text::apply(state.as_deref(), &update)?;
            conn.execute(
                &format!(
                    "UPDATE \"{}\" SET \"{column}\" = ?1, \"{crdt}\" = ?2 WHERE \"{ID_COLUMN}\" = ?3",
                    table.name
                ),
                params![text, state, row],
            )?;
            recompute_if_trashed(conn, table, row)?;
        }
        Op::Delete { .. } => match table.delete_policy {
            DeletePolicy::DeleteWins => remove_row(conn, &table.name, row)?,
            DeletePolicy::Trash | DeletePolicy::EditWins => recompute_deletion(conn, table, row)?,
        },
        Op::Restore => recompute_deletion(conn, table, row)?,
    }
    Ok(Outcome::Applied)
}

/// Rejects remote changes that do not fit the declared schema.
fn validate_remote(table: &TableDef, op: &Op) -> Result<()> {
    let bad = |what: &str| Err(Error::Corrupt(format!("{what} on `{}`", table.name)));
    match op {
        Op::Set { column, value, .. } => {
            let col = table.column(column)?;
            if col.rule != MergeRule::Lww {
                return bad("set on a non-lww column");
            }
            col.check_value(&table.name, value)
        }
        Op::Increment { column, delta } => {
            let col = table.column(column)?;
            if col.rule != MergeRule::Counter {
                return bad("increment on a non-counter column");
            }
            col.check_value(&table.name, delta)
        }
        Op::TextEdit { column, .. } => {
            if table.column(column)?.rule != MergeRule::Text {
                return bad("text edit on a non-text column");
            }
            Ok(())
        }
        Op::Delete { .. } if table.kind == TableKind::Append => bad("delete"),
        Op::Restore if table.delete_policy == DeletePolicy::DeleteWins => bad("restore"),
        _ => Ok(()),
    }
}

/// Derives whether a row is live or in trash from all its delete/restore
/// ops and edits. The result depends only on the set of changes, not on the
/// order they arrived, so every device converges.
fn recompute_deletion(conn: &Connection, table: &TableDef, row: &str) -> Result<()> {
    let mut latest_state: Option<(Hlc, Op)> = None;
    let mut edits: Vec<Hlc> = Vec::new();
    {
        let mut stmt =
            conn.prepare("SELECT hlc, op FROM _pp_changes WHERE tbl = ?1 AND row_id = ?2")?;
        let rows = stmt.query_map(params![table.name, row], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for entry in rows {
            let (hlc, op) = entry?;
            let hlc: Hlc = hlc.parse()?;
            let op: Op = serde_json::from_str(&op)?;
            if op.is_edit() {
                edits.push(hlc);
            } else if matches!(op, Op::Delete { .. } | Op::Restore)
                && latest_state.as_ref().is_none_or(|(h, _)| hlc > *h)
            {
                latest_state = Some((hlc, op));
            }
        }
    }

    let (state, deleted) = match latest_state {
        Some((delete_hlc, Op::Delete { seen })) => {
            let concurrent: Vec<&Hlc> = edits
                .iter()
                .filter(|e| e.device != delete_hlc.device)
                .filter(|e| seen.get(&e.device).is_none_or(|s| *e > s))
                .collect();
            let kind = match table.delete_policy {
                DeletePolicy::EditWins => "restored_by_edit",
                _ => "edited_in_trash",
            };
            for edit in &concurrent {
                conn.execute(
                    "INSERT OR IGNORE INTO _pp_notices(tbl, row_id, kind, hlc) VALUES(?1, ?2, ?3, ?4)",
                    params![table.name, row, kind, edit.to_string()],
                )?;
            }
            if table.delete_policy == DeletePolicy::EditWins && !concurrent.is_empty() {
                (RowState::Live, None)
            } else {
                (RowState::Trashed, Some(delete_hlc))
            }
        }
        _ => (RowState::Live, None),
    };

    conn.execute(
        &format!(
            "UPDATE \"{}\" SET \"{DELETED_COLUMN}\" = ?1 WHERE \"{ID_COLUMN}\" = ?2",
            table.name
        ),
        params![deleted.as_ref().map(Hlc::to_string), row],
    )?;
    conn.execute(
        "UPDATE _pp_rows SET state = ?1, deleted_at_ms = ?2 WHERE tbl = ?3 AND row_id = ?4",
        params![
            state.as_str(),
            deleted.map(|d| d.wall_ms as i64),
            table.name,
            row
        ],
    )?;
    Ok(())
}

fn recompute_if_trashed(conn: &Connection, table: &TableDef, row: &str) -> Result<()> {
    if row_state(conn, &table.name, row)? == Some(RowState::Trashed) {
        recompute_deletion(conn, table, row)?;
    }
    Ok(())
}

fn row_state(conn: &Connection, table: &str, row: &str) -> Result<Option<RowState>> {
    let state: Option<String> = conn
        .query_row(
            "SELECT state FROM _pp_rows WHERE tbl = ?1 AND row_id = ?2",
            params![table, row],
            |r| r.get(0),
        )
        .optional()?;
    state.map(|s| RowState::parse(&s)).transpose()
}

fn ensure_row(conn: &Connection, table: &str, row: &str) -> Result<()> {
    if row_state(conn, table, row)?.is_none() {
        conn.execute(
            &format!("INSERT INTO \"{table}\"(\"{ID_COLUMN}\") VALUES(?1)"),
            [row],
        )?;
        conn.execute(
            "INSERT INTO _pp_rows(tbl, row_id, state) VALUES(?1, ?2, 'live')",
            params![table, row],
        )?;
    }
    Ok(())
}

/// Terminal removal: the row and its cell timestamps go; the tombstone stays
/// so late edits for it are ignored.
fn remove_row(conn: &Connection, table: &str, row: &str) -> Result<()> {
    conn.execute(
        &format!("DELETE FROM \"{table}\" WHERE \"{ID_COLUMN}\" = ?1"),
        [row],
    )?;
    conn.execute(
        "DELETE FROM _pp_cells WHERE tbl = ?1 AND row_id = ?2",
        params![table, row],
    )?;
    conn.execute(
        "INSERT INTO _pp_rows(tbl, row_id, state) VALUES(?1, ?2, 'gone')
         ON CONFLICT(tbl, row_id) DO UPDATE SET state = 'gone', deleted_at_ms = NULL",
        params![table, row],
    )?;
    Ok(())
}

fn cell_hlc(conn: &Connection, table: &str, row: &str, column: &str) -> Result<Option<Hlc>> {
    let hlc: Option<String> = conn
        .query_row(
            "SELECT hlc FROM _pp_cells WHERE tbl = ?1 AND row_id = ?2 AND col = ?3",
            params![table, row, column],
            |r| r.get(0),
        )
        .optional()?;
    hlc.map(|h| h.parse()).transpose()
}

fn read_blob(conn: &Connection, table: &str, row: &str, column: &str) -> Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row(
            &format!("SELECT \"{column}\" FROM \"{table}\" WHERE \"{ID_COLUMN}\" = ?1"),
            [row],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()?
        .flatten())
}

fn read_cell(conn: &Connection, table: &str, row: &str, column: &str) -> Result<Value> {
    let value = conn
        .query_row(
            &format!("SELECT \"{column}\" FROM \"{table}\" WHERE \"{ID_COLUMN}\" = ?1"),
            [row],
            |r| Ok(from_sql(r.get_ref(0)?)),
        )
        .optional()?;
    Ok(value.unwrap_or(Value::Null))
}

fn record_conflict(
    conn: &Connection,
    table: &str,
    row: &str,
    column: &str,
    kept: (&Value, &Hlc),
    lost: (&Value, &Hlc),
    now_ms: u64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO _pp_conflicts(tbl, row_id, col, kept_value, kept_hlc, lost_value, lost_hlc, recorded_ms)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            table,
            row,
            column,
            serde_json::to_string(kept.0)?,
            kept.1.to_string(),
            serde_json::to_string(lost.0)?,
            lost.1.to_string(),
            now_ms as i64
        ],
    )?;
    Ok(())
}

/// Per-device highest timestamp among a row's changes.
fn row_seen(conn: &Connection, table: &str, row: &str) -> Result<VersionVector> {
    let mut stmt = conn.prepare(
        "SELECT device, MAX(hlc) FROM _pp_changes WHERE tbl = ?1 AND row_id = ?2 GROUP BY device",
    )?;
    let rows = stmt.query_map(params![table, row], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out = VersionVector::new();
    for entry in rows {
        let (device, hlc) = entry?;
        out.insert(DeviceId(device), hlc.parse()?);
    }
    Ok(out)
}

struct JsonBudget(usize);
impl std::io::Write for JsonBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(std::io::Error::other("JSON budget exceeded"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
