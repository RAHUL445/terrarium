use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("storage: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("encoding: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid schema: {0}")]
    Schema(String),
    #[error("migration not allowed: {0}")]
    Migration(String),
    #[error("schema downgrade from version {stored} to {requested} is not allowed")]
    Downgrade { stored: u32, requested: u32 },
    #[error("unknown table `{0}`")]
    UnknownTable(String),
    #[error("unknown column `{table}.{column}`")]
    UnknownColumn { table: String, column: String },
    #[error("value for `{table}.{column}` must be {expected}")]
    Type {
        table: String,
        column: String,
        expected: &'static str,
    },
    #[error("row `{row}` not found in `{table}`")]
    NotFound { table: String, row: String },
    #[error("row `{row}` in `{table}` is in trash; restore it first")]
    InTrash { table: String, row: String },
    #[error("table `{0}` is append-only; only inserts are allowed")]
    AppendOnly(String),
    #[error("column `{table}.{column}` is a counter; use increment")]
    IsCounter { table: String, column: String },
    #[error("column `{table}.{column}` is not a counter")]
    NotCounter { table: String, column: String },
    #[error("operation not allowed by delete policy of `{table}`: {reason}")]
    Policy { table: String, reason: &'static str },
    #[error("plugin queries must be read-only SELECT statements on plugin tables")]
    ReadOnlyQuery,
    #[error("plugin query exceeds resource limits: {0}")]
    QueryLimit(&'static str),
    #[error("hlc: {0}")]
    Hlc(String),
    #[error("corrupt store: {0}")]
    Corrupt(String),
}
