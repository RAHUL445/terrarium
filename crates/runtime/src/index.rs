//! Host-only, local search indexes rebuilt from plugin stores and declared
//! queries. These databases are neither synced nor backed up.
use crate::{Manifest, PluginHost, Result, RuntimeError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub(crate) const INDEX_SCHEMA_VERSION: i64 = 2;
/// Row ids per index query.
pub const INDEX_CHUNK: usize = 500;
const MAX_TITLE: usize = 500;
const MAX_SUBTITLE: usize = 300;
const MAX_KEYWORDS: usize = 2_000;
const MAX_BODY: usize = 20_000;
const DDL: &str = "
CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS docs(rowid INTEGER PRIMARY KEY, plugin TEXT NOT NULL,
    tbl TEXT NOT NULL, item TEXT NOT NULL, title TEXT NOT NULL, subtitle TEXT,
    kind TEXT NOT NULL, changed_ms INTEGER NOT NULL, UNIQUE(plugin, item));
CREATE VIRTUAL TABLE IF NOT EXISTS docs_fts USING fts5(title, keywords, body,
    content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2');
CREATE TABLE IF NOT EXISTS mentions(doc INTEGER NOT NULL, to_ref TEXT NOT NULL, PRIMARY KEY(doc, to_ref));
CREATE INDEX IF NOT EXISTS mentions_to ON mentions(to_ref);
CREATE TABLE IF NOT EXISTS cursors(plugin TEXT PRIMARY KEY, lseq INTEGER NOT NULL,
    fingerprint TEXT NOT NULL, rebuild_entry INTEGER, rebuild_after TEXT NOT NULL DEFAULT '');
";
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Doc {
    pub plugin: String,
    pub table: String,
    pub item: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: String,
    pub keywords: String,
    pub body: String,
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cursor {
    pub lseq: i64,
    pub fingerprint: String,
    /// Entry being rebuilt; None when rebuilding is complete.
    pub rebuild_entry: Option<usize>,
    pub rebuild_after: String,
}
/// Result of one catch-up call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct IndexProgress {
    pub rows: usize,
    pub done: bool,
}
pub struct SearchIndex {
    dir: PathBuf,
    conns: BTreeMap<String, Connection>,
}

fn sql<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|e| RuntimeError::Core(e.into()))
}
fn open_index(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)",
    )?;
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if version
        .as_deref()
        .is_some_and(|v| v != INDEX_SCHEMA_VERSION.to_string())
    {
        conn.execute_batch("DROP TABLE IF EXISTS docs; DROP TABLE IF EXISTS docs_fts; DROP TABLE IF EXISTS cursors; DROP TABLE IF EXISTS mentions;")?;
    }
    conn.execute_batch(DDL)?;
    conn.execute("INSERT INTO meta(key, value) VALUES('schema_version', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [INDEX_SCHEMA_VERSION.to_string()])?;
    Ok(conn)
}
/// Distinct refs in `[label](terrarium:<ref>)` links in `text`, at most 100.
pub(crate) fn mentions_in(text: &str) -> Vec<String> {
    const MARKER: &str = "](terrarium:";
    let mut refs = Vec::new();
    let mut seen = BTreeSet::new();
    for (start, _) in text.match_indices(MARKER) {
        let tail = &text[start + MARKER.len()..];
        for (count, (end, c)) in tail.char_indices().enumerate() {
            if count > 200 || c.is_whitespace() {
                break;
            }
            if c == ')' {
                let candidate = &tail[..end];
                if crate::ItemRef::parse(candidate).is_some() && seen.insert(candidate) {
                    refs.push(candidate.to_string());
                    if refs.len() == 100 {
                        return refs;
                    }
                }
                break;
            }
        }
    }
    refs
}

fn save_cursor(conn: &Connection, plugin: &str, c: &Cursor) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO cursors(plugin, lseq, fingerprint, rebuild_entry, rebuild_after) VALUES(?1, ?2, ?3, ?4, ?5) ON CONFLICT(plugin) DO UPDATE SET lseq=excluded.lseq, fingerprint=excluded.fingerprint, rebuild_entry=excluded.rebuild_entry, rebuild_after=excluded.rebuild_after", params![plugin, c.lseq, c.fingerprint, c.rebuild_entry.map(|e| e as i64), c.rebuild_after])?;
    Ok(())
}
impl SearchIndex {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            conns: BTreeMap::new(),
        }
    }
    fn conn(&mut self, space: &str) -> Result<&mut Connection> {
        if space.is_empty()
            || space.len() > 64
            || !space
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(RuntimeError::Manifest(format!(
                "index: bad space id `{space}`"
            )));
        }
        // A deleted local index must reopen and rebuild on the next step.
        if self.conns.contains_key(space) && !self.dir.join(format!("{space}.db")).exists() {
            self.conns.remove(space);
        }
        if !self.conns.contains_key(space) {
            std::fs::create_dir_all(&self.dir).map_err(|e| RuntimeError::io(&self.dir, e))?;
            let path = self.dir.join(format!("{space}.db"));
            let conn = match open_index(&path) {
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if matches!(
                        e.code,
                        rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt
                    ) =>
                {
                    std::fs::remove_file(&path).map_err(|e| RuntimeError::io(&path, e))?;
                    sql(open_index(&path))?
                }
                r => sql(r)?,
            };
            self.conns.insert(space.to_string(), conn);
        }
        Ok(self.conns.get_mut(space).expect("opened"))
    }
    pub(crate) fn close(&mut self, space: &str) {
        self.conns.remove(space);
    }
    pub(crate) fn cursor(&mut self, space: &str, plugin: &str) -> Result<Option<Cursor>> {
        sql(self.conn(space)?.query_row("SELECT lseq, fingerprint, rebuild_entry, rebuild_after FROM cursors WHERE plugin=?1", [plugin], |r| Ok(Cursor { lseq:r.get(0)?, fingerprint:r.get(1)?, rebuild_entry:r.get::<_,Option<i64>>(2)?.map(|e|e as usize), rebuild_after:r.get(3)? })).optional())
    }
    pub(crate) fn begin_rebuild(
        &mut self,
        space: &str,
        plugin: &str,
        fingerprint: &str,
        lseq: i64,
    ) -> Result<()> {
        let tx = sql(self.conn(space)?.transaction())?;
        sql(tx.execute(
            "DELETE FROM docs_fts WHERE rowid IN (SELECT rowid FROM docs WHERE plugin=?1)",
            [plugin],
        ))?;
        sql(tx.execute(
            "DELETE FROM mentions WHERE doc IN (SELECT rowid FROM docs WHERE plugin=?1)",
            [plugin],
        ))?;
        sql(tx.execute("DELETE FROM docs WHERE plugin=?1", [plugin]))?;
        sql(save_cursor(
            &tx,
            plugin,
            &Cursor {
                lseq,
                fingerprint: fingerprint.into(),
                rebuild_entry: Some(0),
                rebuild_after: String::new(),
            },
        ))?;
        sql(tx.commit())
    }
    pub(crate) fn apply(
        &mut self,
        space: &str,
        plugin: &str,
        upserts: &[Doc],
        removes: &[String],
        cursor: &Cursor,
        now_ms: i64,
    ) -> Result<()> {
        let tx = sql(self.conn(space)?.transaction())?;
        for item in removes {
            let id: Option<i64> = sql(tx
                .query_row(
                    "SELECT rowid FROM docs WHERE plugin=?1 AND item=?2",
                    params![plugin, item],
                    |r| r.get(0),
                )
                .optional())?;
            if let Some(id) = id {
                sql(tx.execute("DELETE FROM docs_fts WHERE rowid=?1", [id]))?;
                sql(tx.execute("DELETE FROM mentions WHERE doc=?1", [id]))?;
                sql(tx.execute("DELETE FROM docs WHERE rowid=?1", [id]))?;
            }
        }
        for d in upserts {
            let id: i64 = sql(tx.query_row("INSERT INTO docs(plugin,tbl,item,title,subtitle,kind,changed_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(plugin,item) DO UPDATE SET tbl=excluded.tbl,title=excluded.title,subtitle=excluded.subtitle,kind=excluded.kind,changed_ms=excluded.changed_ms RETURNING rowid",params![d.plugin,d.table,d.item,d.title,d.subtitle,d.kind,now_ms],|r|r.get(0)))?;
            sql(tx.execute("DELETE FROM mentions WHERE doc=?1", [id]))?;
            let own_ref = format!("{}/{}/{}", d.plugin, d.table, d.item);
            for to_ref in mentions_in(&format!("{}\n{}\n{}", d.title, d.keywords, d.body)) {
                if to_ref != own_ref {
                    sql(tx.execute(
                        "INSERT INTO mentions(doc,to_ref) VALUES(?1,?2)",
                        params![id, to_ref],
                    ))?;
                }
            }
            sql(tx.execute("DELETE FROM docs_fts WHERE rowid=?1", [id]))?;
            sql(tx.execute(
                "INSERT INTO docs_fts(rowid,title,keywords,body) VALUES(?1,?2,?3,?4)",
                params![id, d.title, d.keywords, d.body],
            ))?;
        }
        sql(save_cursor(&tx, plugin, cursor))?;
        sql(tx.commit())
    }
    pub(crate) fn forget(&mut self, space: &str, plugin: &str) -> Result<()> {
        let tx = sql(self.conn(space)?.transaction())?;
        sql(tx.execute(
            "DELETE FROM docs_fts WHERE rowid IN (SELECT rowid FROM docs WHERE plugin=?1)",
            [plugin],
        ))?;
        sql(tx.execute(
            "DELETE FROM mentions WHERE doc IN (SELECT rowid FROM docs WHERE plugin=?1)",
            [plugin],
        ))?;
        sql(tx.execute("DELETE FROM docs WHERE plugin=?1", [plugin]))?;
        sql(tx.execute("DELETE FROM cursors WHERE plugin=?1", [plugin]))?;
        sql(tx.commit())
    }
    /// Refs of indexed items in `space` that mention `to`, most recent first, at most 200.
    pub(crate) fn mentioning(&mut self, space: &str, to: &str) -> Result<Vec<String>> {
        let mut stmt = sql(self.conn(space)?.prepare(
            "SELECT d.plugin,d.tbl,d.item FROM mentions m JOIN docs d ON d.rowid=m.doc WHERE m.to_ref=?1 ORDER BY d.changed_ms DESC LIMIT 200",
        ))?;
        let rows = sql(stmt.query_map([to], |r| {
            Ok(format!(
                "{}/{}/{}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?
            ))
        }))?;
        sql(rows.collect())
    }
    pub(crate) fn items(&mut self, space: &str, plugin: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = sql(self
            .conn(space)?
            .prepare("SELECT item,kind FROM docs WHERE plugin=?1 ORDER BY item"))?;
        let rows = sql(stmt.query_map([plugin], |r| Ok((r.get(0)?, r.get(1)?))))?;
        sql(rows.collect())
    }
}
pub(crate) fn fingerprint(m: &Manifest) -> Option<String> {
    if m.index.is_empty() {
        return None;
    }
    let mut parts = vec![INDEX_SCHEMA_VERSION.to_string(), m.version.clone()];
    for e in &m.index {
        let q = &m.queries[&e.query];
        parts.extend([
            e.table.clone(),
            e.query.clone(),
            e.kind.clone(),
            q.sql.clone(),
        ]);
        parts.extend(q.params.clone());
    }
    Some(format!("{:x}", Sha256::digest(parts.join("\n").as_bytes())))
}
fn text(value: Option<&Value>, max: usize) -> String {
    let s = match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    s.chars().take(max).collect()
}
fn read_entry(
    host: &PluginHost,
    space: &str,
    m: &Manifest,
    entry: usize,
    ids: &[String],
) -> Result<(Vec<Doc>, Vec<String>)> {
    let e = &m.index[entry];
    let rows = match host.query_in(
        space,
        &m.id,
        &e.query,
        &Map::from_iter([("ids".into(), json!(ids))]),
    ) {
        Ok(rows) => rows,
        Err(RuntimeError::Core(terrarium_core::Error::QueryLimit(what))) => {
            if ids.len() > 1 {
                let (mut a, mut removes) =
                    read_entry(host, space, m, entry, &ids[..ids.len() / 2])?;
                let (b, rest) = read_entry(host, space, m, entry, &ids[ids.len() / 2..])?;
                a.extend(b);
                removes.extend(rest);
                return Ok((a, removes));
            }
            eprintln!("index: {}/{}: skipped one row ({what})", m.id, e.table);
            return Ok((vec![], ids.to_vec()));
        }
        Err(e) => return Err(e),
    };
    let mut docs = Vec::new();
    let mut seen = BTreeSet::new();
    for row in rows {
        let Some(id) = row
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| ids.iter().any(|i| i == id))
        else {
            continue;
        };
        seen.insert(id.to_string());
        let subtitle = text(row.get("subtitle"), MAX_SUBTITLE);
        docs.push(Doc {
            plugin: m.id.clone(),
            table: e.table.clone(),
            item: id.into(),
            title: text(row.get("title"), MAX_TITLE),
            subtitle: (!subtitle.is_empty()).then_some(subtitle),
            kind: e.kind.clone(),
            keywords: text(row.get("keywords"), MAX_KEYWORDS),
            body: text(row.get("body"), MAX_BODY),
        });
    }
    Ok((
        docs,
        ids.iter()
            .filter(|id| !seen.contains(*id))
            .cloned()
            .collect(),
    ))
}
pub(crate) fn catch_up(
    index: &mut SearchIndex,
    host: &PluginHost,
    space: &str,
    plugin: &str,
    budget: usize,
) -> Result<IndexProgress> {
    let Some(m) = host.manifest(plugin) else {
        return Ok(IndexProgress {
            rows: 0,
            done: true,
        });
    };
    let plugin = m.id.as_str();
    // Entries indexed under a former id are rebuilt under the current one.
    for former in &m.former_ids {
        if index.cursor(space, former)?.is_some() {
            index.forget(space, former)?;
        }
    }
    let Some(fp) = fingerprint(m) else {
        if index.cursor(space, plugin)?.is_some() {
            index.forget(space, plugin)?;
        }
        return Ok(IndexProgress {
            rows: 0,
            done: true,
        });
    };
    let store = host.store_in(space, plugin)?;
    let last = store.last_lseq()?;
    let mut cursor = index.cursor(space, plugin)?;
    if cursor
        .as_ref()
        .is_none_or(|c| c.fingerprint != fp || c.lseq > last)
    {
        index.begin_rebuild(space, plugin, &fp, last)?;
        cursor = index.cursor(space, plugin)?;
    }
    let mut cursor = cursor.expect("cursor created");
    let mut rows = 0;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    while rows < budget {
        let changes = store.changed_rows(cursor.lseq, INDEX_CHUNK.min(budget - rows))?;
        if changes.is_empty() {
            break;
        }
        let mut groups: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
        for (_, table, id) in &changes {
            if let Some(e) = m.index.iter().position(|e| e.table == *table) {
                groups.entry(e).or_default().insert(id.clone());
            }
        }
        let mut docs = Vec::new();
        let mut removes = Vec::new();
        for (e, ids) in groups {
            let (a, b) = read_entry(host, space, m, e, &ids.into_iter().collect::<Vec<_>>())?;
            docs.extend(a);
            removes.extend(b);
        }
        cursor.lseq = changes.last().expect("not empty").0;
        index.apply(space, plugin, &docs, &removes, &cursor, now_ms)?;
        rows += changes.len();
    }
    while rows < budget {
        let Some(e) = cursor.rebuild_entry else {
            break;
        };
        let ids = store.row_ids_after(
            &m.index[e].table,
            &cursor.rebuild_after,
            INDEX_CHUNK.min(budget - rows),
        )?;
        if ids.is_empty() {
            cursor.rebuild_entry = (e + 1 < m.index.len()).then_some(e + 1);
            cursor.rebuild_after.clear();
            index.apply(space, plugin, &[], &[], &cursor, now_ms)?;
        } else {
            let (docs, removes) = read_entry(host, space, m, e, &ids)?;
            cursor.rebuild_after = ids.last().expect("not empty").clone();
            index.apply(space, plugin, &docs, &removes, &cursor, now_ms)?;
            rows += ids.len();
        }
    }
    Ok(IndexProgress {
        rows,
        done: cursor.rebuild_entry.is_none() && cursor.lseq >= last,
    })
}

/// The words retained for FTS and snippets, with user operators kept as text.
pub(crate) fn query_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .take(12)
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(str::to_lowercase)
        .collect()
}
/// Quoted words joined with AND; the last word is a prefix. None when
/// nothing searchable remains.
pub(crate) fn fts_query(text: &str) -> Option<String> {
    let mut words: Vec<String> = text
        .split_whitespace()
        .take(12)
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
        .collect();
    words.last_mut()?.push('*');
    Some(words.join(" AND "))
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IndexHit {
    pub plugin: String,
    pub table: String,
    pub item: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: String,
    pub score: f64,
    pub changed_ms: i64,
}
impl SearchIndex {
    /// Best matches first, with ties going to the most recently indexed item.
    pub(crate) fn search(
        &mut self,
        space: &str,
        text: &str,
        plugins: &[String],
        limit: usize,
    ) -> Result<Vec<IndexHit>> {
        let Some(query) = fts_query(text) else {
            return Ok(vec![]);
        };
        if plugins.is_empty() {
            return Ok(vec![]);
        }
        let conn = self.conn(space)?;
        let result = (|| -> rusqlite::Result<Vec<IndexHit>> {
            let mut stmt=conn.prepare("SELECT d.plugin,d.tbl,d.item,d.title,d.subtitle,d.kind,d.changed_ms,bm25(docs_fts,5.0,3.0,1.0) AS score FROM docs_fts JOIN docs d ON d.rowid=docs_fts.rowid WHERE docs_fts MATCH ?1 AND d.plugin IN (SELECT value FROM json_each(?2)) ORDER BY score,d.changed_ms DESC LIMIT ?3")?;
            let rows = stmt.query_map(
                params![
                    query,
                    serde_json::to_string(plugins).expect("string list"),
                    limit as i64
                ],
                |r| {
                    Ok(IndexHit {
                        plugin: r.get(0)?,
                        table: r.get(1)?,
                        item: r.get(2)?,
                        title: r.get(3)?,
                        subtitle: r.get(4)?,
                        kind: r.get(5)?,
                        changed_ms: r.get(6)?,
                        score: r.get(7)?,
                    })
                },
            )?;
            rows.collect()
        })();
        match result {
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.contains("fts5: syntax error") =>
            {
                Ok(vec![])
            }
            result => sql(result),
        }
    }
}

impl SearchIndex {
    pub(crate) fn ready(&mut self, space: &str, plugin: &str, fingerprint: &str) -> Result<bool> {
        Ok(self
            .cursor(space, plugin)?
            .is_some_and(|c| c.fingerprint == fingerprint && c.rebuild_entry.is_none()))
    }
}
/// Up to 160 characters around a matching word, with collapsed whitespace
/// and ellipses where the body is cut. Never slices inside a Unicode character.
pub(crate) fn snippet(body: &str, words: &[String]) -> Option<String> {
    let text = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = text.chars().collect();
    let mut folded = String::new();
    let mut origins = Vec::new();
    for (i, c) in chars.iter().enumerate() {
        for lower in c.to_lowercase() {
            origins.extend(std::iter::repeat_n(i, lower.len_utf8()));
            folded.push(lower);
        }
    }
    let found = words
        .iter()
        .filter(|w| !w.is_empty())
        .find_map(|w| folded.find(&w.to_lowercase()))?;
    let at = *origins.get(found)?;
    let mut start = at.saturating_sub(40);
    if start > 0 {
        while start < at && chars[start] != ' ' {
            start += 1;
        }
        if chars[start] == ' ' {
            start += 1;
        }
    }
    let end = (start + 160).min(chars.len());
    Some(format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn doc(item: &str, title: &str) -> Doc {
        Doc {
            plugin: "todo".into(),
            table: "tasks".into(),
            item: item.into(),
            title: title.into(),
            subtitle: None,
            kind: "Task".into(),
            keywords: String::new(),
            body: String::new(),
        }
    }
    fn cursor() -> Cursor {
        Cursor {
            lseq: 0,
            fingerprint: "test".into(),
            rebuild_entry: None,
            rebuild_after: String::new(),
        }
    }
    fn matches(index: &mut SearchIndex, term: &str) -> i64 {
        index
            .conn("personal")
            .unwrap()
            .query_row(
                "SELECT count(*) FROM docs_fts WHERE docs_fts MATCH ?1",
                [term],
                |r| r.get(0),
            )
            .unwrap()
    }
    #[test]
    fn mentions_are_found_in_markdown() {
        assert_eq!(mentions_in("a [x](terrarium:todo/tasks/t1) b [y](terrarium:todo/tasks/t1) [z](https://e.com) [w](terrarium:bad)"), ["todo/tasks/t1"]);
        assert_eq!(
            mentions_in("東京 [x](terrarium:notes/notes/n1) [y](terrarium:todo/tasks/t2)"),
            ["notes/notes/n1", "todo/tasks/t2"]
        );
        for bad in [
            "[x](terrarium:todo/tasks/a b)",
            "[x](terrarium:todo/tasks/a\nb)",
            "[x](terrarium:../../etc)",
            "[x](terrarium:todo/tasks/é)",
            "[x](terrarium:todo/tasks/t1",
            "terrarium:todo/tasks/t1",
        ] {
            assert!(mentions_in(bad).is_empty(), "{bad}");
        }
        assert!(mentions_in(&format!("[x](terrarium:{})", "a".repeat(201))).is_empty());
        let many = (0..110)
            .map(|n| format!("[x](terrarium:todo/tasks/t{n})"))
            .collect::<String>();
        let found = mentions_in(&many);
        assert_eq!(found.len(), 100);
        assert_eq!(found[99], "todo/tasks/t99");
    }
    #[test]
    fn apply_records_and_replaces_mentions() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        let mut note = doc("n1", "Note");
        note.plugin = "notes".into();
        note.table = "notes".into();
        note.body = "See [T](terrarium:todo/tasks/t1)".into();
        index
            .apply(
                "personal",
                "notes",
                std::slice::from_ref(&note),
                &[],
                &cursor(),
                1,
            )
            .unwrap();
        assert_eq!(
            index.mentioning("personal", "todo/tasks/t1").unwrap(),
            ["notes/notes/n1"]
        );
        assert!(index
            .mentioning("work", "todo/tasks/t1")
            .unwrap()
            .is_empty());
        note.body.clear();
        index
            .apply(
                "personal",
                "notes",
                std::slice::from_ref(&note),
                &[],
                &cursor(),
                2,
            )
            .unwrap();
        assert!(index
            .mentioning("personal", "todo/tasks/t1")
            .unwrap()
            .is_empty());
        note.title = "[T](terrarium:todo/tasks/t1)".into();
        note.keywords = "[U](terrarium:todo/tasks/t2) [T](terrarium:todo/tasks/t1)".into();
        note.body = "[Self](terrarium:notes/notes/n1)".into();
        index
            .apply(
                "personal",
                "notes",
                std::slice::from_ref(&note),
                &[],
                &cursor(),
                3,
            )
            .unwrap();
        for target in ["todo/tasks/t1", "todo/tasks/t2"] {
            assert_eq!(
                index.mentioning("personal", target).unwrap(),
                ["notes/notes/n1"]
            );
        }
        assert!(index
            .mentioning("personal", "notes/notes/n1")
            .unwrap()
            .is_empty());
        index
            .apply("personal", "notes", &[], &["n1".into()], &cursor(), 4)
            .unwrap();
        assert!(index
            .mentioning("personal", "todo/tasks/t1")
            .unwrap()
            .is_empty());
        assert_eq!(
            index
                .conn("personal")
                .unwrap()
                .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        // Reused doc rowids must never inherit deleted mentions.
        index
            .apply("personal", "todo", &[doc("t3", "Plain")], &[], &cursor(), 5)
            .unwrap();
        assert!(index
            .mentioning("personal", "todo/tasks/t1")
            .unwrap()
            .is_empty());
    }
    #[test]
    fn rebuild_and_forget_remove_only_the_plugins_mentions() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        let task = doc("t1", "[X](terrarium:notes/notes/n1)");
        let mut other = task.clone();
        other.plugin = "other".into();
        index
            .apply(
                "personal",
                "todo",
                std::slice::from_ref(&task),
                &[],
                &cursor(),
                1,
            )
            .unwrap();
        index
            .apply("personal", "other", &[other], &[], &cursor(), 2)
            .unwrap();
        index.begin_rebuild("personal", "todo", "new", 0).unwrap();
        assert_eq!(
            index.mentioning("personal", "notes/notes/n1").unwrap(),
            ["other/tasks/t1"]
        );
        index
            .apply("personal", "todo", &[task], &[], &cursor(), 3)
            .unwrap();
        index.forget("personal", "todo").unwrap();
        assert_eq!(
            index.mentioning("personal", "notes/notes/n1").unwrap(),
            ["other/tasks/t1"]
        );
        assert_eq!(
            index
                .conn("personal")
                .unwrap()
                .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn backlinks_are_recent_first_and_limited_to_200() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        for n in 0..205 {
            index
                .apply(
                    "personal",
                    "todo",
                    &[doc(&format!("t{n}"), "[X](terrarium:notes/notes/n1)")],
                    &[],
                    &cursor(),
                    n,
                )
                .unwrap();
        }
        let refs = index.mentioning("personal", "notes/notes/n1").unwrap();
        assert_eq!(refs.len(), 200);
        assert_eq!(refs[0], "todo/tasks/t204");
        assert_eq!(refs[199], "todo/tasks/t5");
    }
    #[test]
    fn a_new_index_has_its_schema() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        let conn = index.conn("personal").unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version, "2");
        for name in ["docs", "docs_fts", "cursors", "mentions", "mentions_to"] {
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name=?1",
                    [name],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        assert!(dir.path().join("personal.db").is_file());
    }
    #[test]
    fn apply_upserts_replaces_and_removes() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        index
            .apply(
                "personal",
                "todo",
                &[doc("a", "Buy desk lamp")],
                &[],
                &cursor(),
                1,
            )
            .unwrap();
        assert_eq!(matches(&mut index, "lamp"), 1);
        index
            .apply(
                "personal",
                "todo",
                &[doc("a", "Buy chair")],
                &[],
                &cursor(),
                2,
            )
            .unwrap();
        assert_eq!(matches(&mut index, "lamp"), 0);
        assert_eq!(matches(&mut index, "chair"), 1);
        assert_eq!(index.items("personal", "todo").unwrap().len(), 1);
        index
            .apply("personal", "todo", &[], &["a".into()], &cursor(), 3)
            .unwrap();
        assert!(index.items("personal", "todo").unwrap().is_empty());
        assert_eq!(matches(&mut index, "chair"), 0);
    }
    #[test]
    fn version_2_rebuilds_old_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        index
            .apply("personal", "todo", &[doc("a", "Lamp")], &[], &cursor(), 1)
            .unwrap();
        drop(index);
        Connection::open(dir.path().join("personal.db"))
            .unwrap()
            .execute_batch(
                "UPDATE meta SET value='1' WHERE key='schema_version'; DROP TABLE mentions;",
            )
            .unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        assert!(index.items("personal", "todo").unwrap().is_empty());
        assert_eq!(
            index
                .conn("personal")
                .unwrap()
                .query_row(
                    "SELECT value FROM meta WHERE key='schema_version'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "2"
        );
        assert_eq!(
            index
                .conn("personal")
                .unwrap()
                .query_row("SELECT count(*) FROM mentions", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn a_corrupt_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("personal.db"), b"not a database").unwrap();
        assert!(SearchIndex::new(dir.path().to_path_buf())
            .conn("personal")
            .is_ok());
    }
    #[test]
    fn bad_space_names_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        for name in ["../x", "", "a/b"] {
            assert!(index.conn(name).is_err());
        }
    }
    #[test]
    fn fingerprint_tracks_version_and_declarations() {
        let base = json!({"id":"sample","name":"Sample","version":"1.0.0","api":crate::API_VERSION,"storage":{"version":1,"tables":[{"name":"items","columns":[{"name":"title","type":"text"}]}]},"queries":{"index_items":{"sql":"SELECT id,title FROM items WHERE id IN (SELECT value FROM json_each(?1))","params":["ids"]}},"index":[{"table":"items","query":"index_items","kind":"Item"}]});
        let m = Manifest::parse(&serde_json::to_vec(&base).unwrap()).unwrap();
        assert_eq!(fingerprint(&m), fingerprint(&m));
        for path in ["version", "kind", "sql"] {
            let mut v = base.clone();
            match path { "version"=>v["version"]=json!("1.0.1"), "kind"=>v["index"][0]["kind"]=json!("Job"), _=>v["queries"]["index_items"]["sql"]=json!("SELECT id,title FROM items WHERE id IN (SELECT value FROM json_each(?1)) ORDER BY id") }
            assert_ne!(
                fingerprint(&m),
                fingerprint(&Manifest::parse(&serde_json::to_vec(&v).unwrap()).unwrap())
            );
        }
        let mut m = m;
        m.index.clear();
        assert_eq!(fingerprint(&m), None);
    }
    #[test]
    fn fts_query_quotes_words_and_prefixes_the_last() {
        assert_eq!(
            fts_query("desk lamp").as_deref(),
            Some("\"desk\" AND \"lamp\"*")
        );
        assert_eq!(query_words("DESK Lamp"), ["desk", "lamp"]);
        assert_eq!(query_words(&vec!["a"; 20].join(" ")).len(), 12);
    }
    #[test]
    fn fts_query_neutralizes_operators() {
        assert_eq!(
            fts_query("lamp OR chair").as_deref(),
            Some("\"lamp\" AND \"OR\" AND \"chair\"*")
        );
        assert_eq!(
            fts_query("say \"hi\"").as_deref(),
            Some("\"say\" AND \"\"\"hi\"\"\"*")
        );
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        for text in ["NEAR(a b)", "title:x", "-x", "a*", "^a"] {
            assert!(fts_query(text).is_some());
            assert!(index.search("personal", text, &["todo".into()], 10).is_ok());
        }
    }
    #[test]
    fn fts_query_of_nothing_is_none() {
        for text in ["", "   ", "- * \""] {
            assert_eq!(fts_query(text), None);
        }
    }
    #[test]
    fn prefix_matches_while_typing() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        index
            .apply(
                "personal",
                "todo",
                &[doc("a", "Desk lamp")],
                &[],
                &cursor(),
                1,
            )
            .unwrap();
        for text in ["lam", "desk la"] {
            assert_eq!(
                index
                    .search("personal", text, &["todo".into()], 10)
                    .unwrap()
                    .len(),
                1
            );
        }
        assert!(index
            .search("personal", "desk chair", &["todo".into()], 10)
            .unwrap()
            .is_empty());
    }
    #[test]
    fn title_beats_body() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        let mut a = doc("a", "Lamp");
        a.body = "x".into();
        let mut b = doc("b", "Note");
        b.body = "lamp lamp".into();
        index
            .apply("personal", "todo", &[a, b], &[], &cursor(), 1)
            .unwrap();
        assert_eq!(
            index
                .search("personal", "lamp", &["todo".into()], 10)
                .unwrap()[0]
                .item,
            "a"
        );
    }
    #[test]
    fn hits_from_several_plugins_interleave() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        index
            .apply("personal", "todo", &[doc("a", "Lamp")], &[], &cursor(), 1)
            .unwrap();
        let mut b = doc("b", "Lamp");
        b.plugin = "notes".into();
        index
            .apply("personal", "notes", &[b], &[], &cursor(), 2)
            .unwrap();
        let hits = index
            .search("personal", "lamp", &["todo".into(), "notes".into()], 10)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].plugin, "notes");
        let hits = index
            .search("personal", "lamp", &["notes".into()], 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].plugin, "notes");
    }
    #[test]
    fn diacritics_and_non_latin_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        index
            .apply(
                "personal",
                "todo",
                &[doc("a", "Café crème"), doc("b", "東京 タワー")],
                &[],
                &cursor(),
                1,
            )
            .unwrap();
        assert_eq!(
            index
                .search("personal", "cafe", &["todo".into()], 10)
                .unwrap()[0]
                .item,
            "a"
        );
        assert_eq!(
            index
                .search("personal", "東京", &["todo".into()], 10)
                .unwrap()[0]
                .item,
            "b"
        );
    }
    #[test]
    fn ties_go_to_recent() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = SearchIndex::new(dir.path().to_path_buf());
        index
            .apply("personal", "todo", &[doc("a", "Lamp")], &[], &cursor(), 1)
            .unwrap();
        index
            .apply("personal", "todo", &[doc("b", "Lamp")], &[], &cursor(), 2)
            .unwrap();
        assert_eq!(
            index
                .search("personal", "lamp", &["todo".into()], 10)
                .unwrap()[0]
                .item,
            "b"
        );
    }
    #[test]
    fn snippet_centers_on_the_word() {
        let body = format!("{}lamp {}", "word ".repeat(40), "word ".repeat(40));
        let s = snippet(&body, &["lamp".into()]).unwrap();
        assert!(s.contains("lamp"));
        assert!(s.starts_with('…'));
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 162);
    }
    #[test]
    fn snippet_is_none_without_a_match() {
        assert_eq!(snippet("nothing here", &["lamp".into()]), None);
    }
    #[test]
    fn snippet_handles_multibyte_text() {
        let body = format!("{}東京 lamp {}", "é ".repeat(100), "東京 ".repeat(100));
        let s = snippet(&body, &["lamp".into()]).unwrap();
        assert!(s.contains("lamp"));
        assert!(s.chars().count() <= 162);
        assert!(snippet("İstanbul lamp", &["lamp".into()])
            .unwrap()
            .contains("lamp"));
    }
}
