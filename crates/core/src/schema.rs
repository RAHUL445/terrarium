//! Plugin storage schema as declared in the manifest, its validation, the
//! additive-only migration check, and DDL generation.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSchema {
    pub version: u32,
    pub tables: Vec<TableDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableDef {
    pub name: String,
    #[serde(default)]
    pub kind: TableKind,
    #[serde(default)]
    pub delete_policy: DeletePolicy,
    pub columns: Vec<ColumnDef>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnDef {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: ColumnType,
    #[serde(default)]
    pub rule: MergeRule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    Text,
    Integer,
    Real,
    Boolean,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MergeRule {
    #[default]
    Lww,
    Counter,
    /// Loro text CRDT: concurrent edits merge character by character. The
    /// CRDT state lives in a hidden `<column>__crdt` blob; the column itself
    /// holds the plain text for reads and search.
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableKind {
    #[default]
    Standard,
    Append,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeletePolicy {
    #[default]
    Trash,
    DeleteWins,
    EditWins,
}

/// Columns every plugin table gets; plugin column names may not collide.
pub const ID_COLUMN: &str = "id";
pub const DELETED_COLUMN: &str = "_deleted";
pub(crate) const CRDT_SUFFIX: &str = "__crdt";

impl PluginSchema {
    pub fn table(&self, name: &str) -> Result<&TableDef> {
        self.tables
            .iter()
            .find(|t| t.name == name)
            .ok_or_else(|| Error::UnknownTable(name.to_string()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.version == 0 {
            return Err(Error::Schema("version must be at least 1".into()));
        }
        let mut tables = HashSet::new();
        for table in &self.tables {
            check_ident(&table.name)?;
            if !tables.insert(&table.name) {
                return Err(Error::Schema(format!("duplicate table `{}`", table.name)));
            }
            let mut columns = HashSet::new();
            for column in &table.columns {
                check_ident(&column.name)?;
                if column.name == ID_COLUMN {
                    return Err(Error::Schema(format!("`{}.id` is reserved", table.name)));
                }
                if !columns.insert(&column.name) {
                    return Err(Error::Schema(format!(
                        "duplicate column `{}.{}`",
                        table.name, column.name
                    )));
                }
                match column.rule {
                    MergeRule::Lww => {}
                    MergeRule::Counter => {
                        if !matches!(column.ty, ColumnType::Integer | ColumnType::Real) {
                            return Err(Error::Schema(format!(
                                "counter `{}.{}` must be integer or real",
                                table.name, column.name
                            )));
                        }
                        if table.kind == TableKind::Append {
                            return Err(Error::Schema(format!(
                                "append-only table `{}` cannot have counter columns",
                                table.name
                            )));
                        }
                    }
                    MergeRule::Text => {
                        if column.ty != ColumnType::Text || table.kind == TableKind::Append {
                            return Err(Error::Schema(format!(
                                "`{}.{}`: the text rule needs a text column in a standard table",
                                table.name, column.name
                            )));
                        }
                    }
                }
                if let Some(default) = &column.default {
                    column.check_value(&table.name, default)?;
                }
            }
        }
        Ok(())
    }

    /// Migrations may only add tables, columns and nothing else: old changes
    /// from other devices must keep meaning the same thing.
    pub fn check_migration(&self, next: &PluginSchema) -> Result<()> {
        next.validate()?;
        if next.version <= self.version {
            return Err(Error::Migration(format!(
                "new version {} must be greater than {}",
                next.version, self.version
            )));
        }
        for old in &self.tables {
            let new = next
                .tables
                .iter()
                .find(|t| t.name == old.name)
                .ok_or_else(|| {
                    Error::Migration(format!("table `{}` was removed or renamed", old.name))
                })?;
            if new.kind != old.kind || new.delete_policy != old.delete_policy {
                return Err(Error::Migration(format!(
                    "table `{}` changed kind or delete policy",
                    old.name
                )));
            }
            for column in &old.columns {
                let same = new.columns.iter().find(|c| c.name == column.name);
                match same {
                    None => {
                        return Err(Error::Migration(format!(
                            "column `{}.{}` was removed or renamed",
                            old.name, column.name
                        )))
                    }
                    Some(c) if c != column => {
                        return Err(Error::Migration(format!(
                            "column `{}.{}` changed type, rule or default",
                            old.name, column.name
                        )))
                    }
                    Some(_) => {}
                }
            }
        }
        Ok(())
    }

    /// DDL that brings a database at `from` (or empty) to this schema.
    pub(crate) fn ddl_from(&self, from: Option<&PluginSchema>) -> Vec<String> {
        let mut out = Vec::new();
        for table in &self.tables {
            let existing = from.and_then(|f| f.tables.iter().find(|t| t.name == table.name));
            match existing {
                None => {
                    let mut cols = vec![
                        format!("\"{ID_COLUMN}\" TEXT PRIMARY KEY NOT NULL"),
                        format!("\"{DELETED_COLUMN}\" TEXT"),
                    ];
                    cols.extend(table.columns.iter().flat_map(ColumnDef::ddl));
                    out.push(format!(
                        "CREATE TABLE \"{}\" ({})",
                        table.name,
                        cols.join(", ")
                    ));
                }
                Some(old) => {
                    for column in &table.columns {
                        if !old.columns.iter().any(|c| c.name == column.name) {
                            for def in column.ddl() {
                                out.push(format!(
                                    "ALTER TABLE \"{}\" ADD COLUMN {def}",
                                    table.name
                                ));
                            }
                        }
                    }
                }
            }
        }
        out
    }
}

impl TableDef {
    pub fn column(&self, name: &str) -> Result<&ColumnDef> {
        self.columns
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| Error::UnknownColumn {
                table: self.name.clone(),
                column: name.to_string(),
            })
    }
}

impl ColumnDef {
    /// Hidden companion column holding a `text` column's CRDT state.
    pub fn crdt_column(&self) -> String {
        format!("{}{CRDT_SUFFIX}", self.name)
    }

    fn ddl(&self) -> Vec<String> {
        let ty = match self.ty {
            ColumnType::Text => "TEXT",
            ColumnType::Integer | ColumnType::Boolean => "INTEGER",
            ColumnType::Real => "REAL",
        };
        let default = match (&self.default, self.rule) {
            (Some(v), _) => format!(" DEFAULT {}", sql_literal(v)),
            (None, MergeRule::Counter) => " NOT NULL DEFAULT 0".to_string(),
            (None, _) => String::new(),
        };
        let mut out = vec![format!("\"{}\" {ty}{default}", self.name)];
        if self.rule == MergeRule::Text {
            out.push(format!("\"{}\" BLOB", self.crdt_column()));
        }
        out
    }

    pub fn check_value(&self, table: &str, value: &Value) -> Result<()> {
        let ok = match (self.ty, value) {
            (_, Value::Null) => self.rule != MergeRule::Counter,
            (ColumnType::Text, Value::String(_)) => true,
            (ColumnType::Integer, Value::Number(n)) => n.is_i64(),
            (ColumnType::Real, Value::Number(_)) => true,
            (ColumnType::Boolean, Value::Bool(_)) => true,
            _ => false,
        };
        if ok {
            Ok(())
        } else {
            Err(Error::Type {
                table: table.to_string(),
                column: self.name.clone(),
                expected: match self.ty {
                    ColumnType::Text => "text or null",
                    ColumnType::Integer => "an integer",
                    ColumnType::Real => "a number",
                    ColumnType::Boolean => "true, false or null",
                },
            })
        }
    }
}

fn check_ident(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = name.len() <= 63
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.contains("__");
    if valid {
        Ok(())
    } else {
        Err(Error::Schema(format!(
            "`{name}`: names must start with a-z, use only a-z, 0-9 and single _ (max 63)"
        )))
    }
}

fn sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Bool(b) => (if *b { "1" } else { "0" }).into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        _ => "NULL".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema(v: Value) -> PluginSchema {
        serde_json::from_value(v).unwrap()
    }

    fn base() -> PluginSchema {
        schema(json!({"version": 1, "tables": [
            {"name": "tasks", "columns": [
                {"name": "title", "type": "text"},
                {"name": "done", "type": "boolean", "default": false},
                {"name": "streak", "type": "integer", "rule": "counter"}
            ]}
        ]}))
    }

    #[test]
    fn valid_schema_passes() {
        base().validate().unwrap();
    }

    #[test]
    fn rejects_reserved_bad_and_unsupported_names() {
        let bad = [
            json!({"version": 1, "tables": [{"name": "t", "columns": [{"name": "id", "type": "text"}]}]}),
            json!({"version": 1, "tables": [{"name": "_pp_x", "columns": []}]}),
            json!({"version": 1, "tables": [{"name": "T", "columns": []}]}),
            json!({"version": 1, "tables": [{"name": "t", "columns": [{"name": "c", "type": "text", "rule": "counter"}]}]}),
            json!({"version": 1, "tables": [{"name": "t", "columns": [{"name": "c", "type": "integer", "rule": "text"}]}]}),
            json!({"version": 1, "tables": [{"name": "t", "kind": "append", "columns": [{"name": "c", "type": "text", "rule": "text"}]}]}),
            json!({"version": 1, "tables": [{"name": "t", "columns": [{"name": "c__crdt", "type": "text"}]}]}),
            json!({"version": 1, "tables": [{"name": "t", "kind": "append", "columns": [{"name": "c", "type": "integer", "rule": "counter"}]}]}),
            json!({"version": 0, "tables": []}),
        ];
        for v in bad {
            assert!(schema(v.clone()).validate().is_err(), "should reject {v}");
        }
    }

    #[test]
    fn additive_migration_allowed() {
        let mut next = base();
        next.version = 2;
        next.tables[0].columns.push(ColumnDef {
            name: "due".into(),
            ty: ColumnType::Text,
            rule: MergeRule::Lww,
            default: None,
        });
        next.tables.push(TableDef {
            name: "tags".into(),
            kind: TableKind::Standard,
            delete_policy: DeletePolicy::DeleteWins,
            columns: vec![],
        });
        base().check_migration(&next).unwrap();
        let ddl = next.ddl_from(Some(&base()));
        assert_eq!(ddl.len(), 2);
        assert!(ddl[0].starts_with("ALTER TABLE \"tasks\" ADD COLUMN \"due\""));
        assert!(ddl[1].starts_with("CREATE TABLE \"tags\""));
    }

    #[test]
    fn destructive_migrations_rejected() {
        let mut dropped = base();
        dropped.version = 2;
        dropped.tables[0].columns.remove(0);
        assert!(base().check_migration(&dropped).is_err());

        let mut retyped = base();
        retyped.version = 2;
        retyped.tables[0].columns[0].ty = ColumnType::Integer;
        assert!(base().check_migration(&retyped).is_err());

        let mut same_version = base();
        same_version.tables[0].columns.push(ColumnDef {
            name: "x".into(),
            ty: ColumnType::Text,
            rule: MergeRule::Lww,
            default: None,
        });
        assert!(base().check_migration(&same_version).is_err());

        let mut policy = base();
        policy.version = 2;
        policy.tables[0].delete_policy = DeletePolicy::DeleteWins;
        assert!(base().check_migration(&policy).is_err());
    }
}
