//! SQLite authorization applies while preparing/executing plugin reads, while
//! host-owned transactions retain access to metadata and migrations.
use crate::{PluginSchema, Result};
use rusqlite::{
    hooks::{AuthAction, Authorization},
    limits::Limit,
    Connection,
};
use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

pub(crate) const MAX_ROWS: usize = 10_000;
pub(crate) const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_SQL_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct State {
    active: AtomicBool,
    bounded: AtomicBool,
    ticks: AtomicU64,
    deadline: AtomicU64,
}

pub(crate) struct ReadPolicy {
    state: Arc<State>,
    origin: Instant,
}

impl ReadPolicy {
    pub fn install(conn: &mut Connection, schema: &PluginSchema) -> Result<Self> {
        // Initialize the two reviewed eponymous JSON modules before enabling
        // read authorization. Their setup consults SQLite's schema internally.
        for sql in [
            "SELECT count(*) FROM json_each('[]')",
            "SELECT count(*) FROM json_tree('[]')",
        ] {
            conn.query_row(sql, [], |_| Ok(()))?;
        }
        let state = Arc::new(State::default());
        let auth = state.clone();
        let tables: BTreeSet<_> = schema
            .tables
            .iter()
            .map(|t| t.name.to_ascii_lowercase())
            .collect();
        conn.authorizer(Some(move |context: rusqlite::hooks::AuthContext<'_>| {
            if !auth.active.load(Ordering::Relaxed) {
                return Authorization::Allow;
            }
            let allowed = match context.action {
                AuthAction::Select | AuthAction::Recursive => true,
                AuthAction::Read {
                    table_name,
                    column_name,
                } => {
                    // SQLite may omit the database for count(*) table reads.
                    let table = table_name.to_ascii_lowercase();
                    ((context.database_name == Some("main")
                        || (context.database_name.is_none() && column_name.is_empty()))
                        && tables.contains(&table))
                        || (matches!(table.as_str(), "json_each" | "json_tree")
                            && matches!(context.database_name, None | Some("main")))
                }
                AuthAction::Function { function_name } => safe_function(function_name),
                _ => false,
            };
            if allowed {
                Authorization::Allow
            } else {
                Authorization::Deny
            }
        }))?;
        let progress = state.clone();
        let origin = Instant::now();
        conn.progress_handler(
            1000,
            Some(move || {
                progress.bounded.load(Ordering::Relaxed)
                    && (progress.ticks.fetch_add(1, Ordering::Relaxed) >= 10_000
                        || origin.elapsed().as_millis() as u64
                            >= progress.deadline.load(Ordering::Relaxed))
            }),
        )?;
        Ok(Self { state, origin })
    }

    pub fn enter<'a>(
        &'a self,
        conn: &'a Connection,
        bounded: bool,
        media: bool,
    ) -> Result<ReadGuard<'a>> {
        // Store is not Sync and queries cannot be nested on one Connection.
        let mut guard = ReadGuard {
            policy: self,
            conn,
            limits: Vec::new(),
        };
        if bounded {
            for (limit, value) in [
                (
                    Limit::SQLITE_LIMIT_LENGTH,
                    if media { 12 * 1024 * 1024 } else { 1024 * 1024 },
                ),
                (Limit::SQLITE_LIMIT_SQL_LENGTH, MAX_SQL_BYTES as i32),
                (Limit::SQLITE_LIMIT_COLUMN, 128),
                (Limit::SQLITE_LIMIT_EXPR_DEPTH, 64),
                (Limit::SQLITE_LIMIT_COMPOUND_SELECT, 32),
                (Limit::SQLITE_LIMIT_VARIABLE_NUMBER, 128),
                (Limit::SQLITE_LIMIT_LIKE_PATTERN_LENGTH, 4096),
            ] {
                guard.limits.push((limit, conn.set_limit(limit, value)?));
            }
        }
        self.state.ticks.store(0, Ordering::Relaxed);
        self.state.deadline.store(
            self.origin.elapsed().as_millis() as u64 + 250,
            Ordering::Relaxed,
        );
        self.state.bounded.store(bounded, Ordering::Relaxed);
        self.state.active.store(true, Ordering::Relaxed);
        Ok(guard)
    }
}

pub(crate) struct ReadGuard<'a> {
    policy: &'a ReadPolicy,
    conn: &'a Connection,
    limits: Vec<(Limit, i32)>,
}

impl Drop for ReadGuard<'_> {
    fn drop(&mut self) {
        self.policy.state.active.store(false, Ordering::Relaxed);
        self.policy.state.bounded.store(false, Ordering::Relaxed);
        for (limit, old) in self.limits.drain(..) {
            let _ = self.conn.set_limit(limit, old);
        }
    }
}

fn safe_function(name: &str) -> bool {
    // Reviewed built-in expressions only; in particular no extension loading,
    // filesystem helpers or pragma table functions.
    matches!(
        name.to_ascii_lowercase().as_str(),
        "abs"
            | "avg"
            | "char"
            | "coalesce"
            | "concat"
            | "concat_ws"
            | "count"
            | "date"
            | "datetime"
            | "format"
            | "glob"
            | "group_concat"
            | "hex"
            | "if"
            | "ifnull"
            | "iif"
            | "instr"
            | "julianday"
            | "length"
            | "like"
            | "likelihood"
            | "likely"
            | "lower"
            | "ltrim"
            | "max"
            | "min"
            | "nullif"
            | "octet_length"
            | "printf"
            | "quote"
            | "random"
            | "randomblob"
            | "replace"
            | "round"
            | "rtrim"
            | "sign"
            | "strftime"
            | "string_agg"
            | "substr"
            | "substring"
            | "sum"
            | "time"
            | "timediff"
            | "total"
            | "trim"
            | "typeof"
            | "unicode"
            | "unhex"
            | "unixepoch"
            | "unlikely"
            | "upper"
            | "zeroblob"
            | "json"
            | "json_array"
            | "json_array_length"
            | "json_error_position"
            | "json_extract"
            | "json_group_array"
            | "json_group_object"
            | "json_insert"
            | "json_object"
            | "json_patch"
            | "json_quote"
            | "json_remove"
            | "json_replace"
            | "json_set"
            | "json_type"
            | "json_valid"
            | "->"
            | "->>"
            | "row_number"
            | "rank"
            | "dense_rank"
            | "percent_rank"
            | "cume_dist"
            | "ntile"
            | "lag"
            | "lead"
            | "first_value"
            | "last_value"
            | "nth_value"
    )
}
