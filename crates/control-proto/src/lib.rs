//! The local control socket: how the `terrarium` command line (and later
//! agents) talk to the running app.
//!
//! The app listens on a Unix socket, `control.sock`, in its data folder,
//! readable only by the user. Each line is one JSON-RPC 2.0 message. A
//! connection starts with `hello` and a client token; every later request
//! runs with that token's scope.
//!
//! Methods:
//!
//! | Method | Params | Result |
//! | --- | --- | --- |
//! | `hello` | `{token, client?}` | `{client, scope, app_version}` |
//! | `status` | `{}` | `{app_version, client, scope, spaces, plugins}` |
//! | `describe` | `{}` | the commands this client may run |
//! | `invoke` | `{kind, id, space?, args?}` | the command's result |
//! | `subscribe` | `{}` | `true`, then `event` notifications: `{name}` |

use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The app's bundle identifier; its data folder is named after it.
pub const APP_ID: &str = "io.github.rahul445.terrarium";
/// The socket's file name inside the app's data folder.
pub const SOCKET_FILE: &str = "control.sock";
/// Every client token starts with this.
pub const TOKEN_PREFIX: &str = "trm_";
/// Longest message either side accepts, in bytes.
pub const MAX_MESSAGE: usize = 4 * 1024 * 1024;

/// JSON-RPC error code for errors from a command (the string code is in
/// `data.code`).
pub const COMMAND_ERROR: i64 = -32000;
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(id: u64, method: &str, params: Value) -> Self {
        Request {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    /// `None` for notifications (`event`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
    /// Set on notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Response {
    pub fn ok(id: u64, result: Value) -> Self {
        Response {
            jsonrpc: "2.0".into(),
            id: Some(id),
            result: Some(result),
            error: None,
            method: None,
            params: None,
        }
    }

    pub fn err(id: Option<u64>, error: RpcError) -> Self {
        Response {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(error),
            method: None,
            params: None,
        }
    }

    /// A notification, e.g. `event` with `{name}`.
    pub fn notify(method: &str, params: Value) -> Self {
        Response {
            jsonrpc: "2.0".into(),
            id: None,
            result: None,
            error: None,
            method: Some(method.into()),
            params: Some(params),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<ErrorData>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorData {
    /// Stable code: `invalid`, `not-found`, `forbidden`, `unauthorized`,
    /// `unknown-space`, `stopped`, `failed`, or a plugin's own code.
    pub code: String,
}

impl RpcError {
    pub fn command(code: &str, message: impl Into<String>) -> Self {
        RpcError {
            code: COMMAND_ERROR,
            message: message.into(),
            data: Some(ErrorData { code: code.into() }),
        }
    }

    pub fn protocol(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// The stable string code, `protocol` for JSON-RPC level errors.
    pub fn kind(&self) -> &str {
        self.data.as_ref().map_or("protocol", |d| d.code.as_str())
    }
}

#[derive(Debug)]
pub enum ReadError {
    Io(std::io::Error),
    TooLong,
    Json(serde_json::Error),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::TooLong => write!(f, "message is longer than {MAX_MESSAGE} bytes"),
            ReadError::Json(e) => write!(f, "not JSON: {e}"),
        }
    }
}

impl std::error::Error for ReadError {}

/// Reads one message. `Ok(None)` at end of stream.
pub fn read_message<T: for<'de> Deserialize<'de>>(
    r: &mut impl BufRead,
) -> Result<Option<T>, ReadError> {
    let mut line = Vec::new();
    let read = std::io::Read::take(r, MAX_MESSAGE as u64 + 1)
        .read_until(b'\n', &mut line)
        .map_err(ReadError::Io)?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > MAX_MESSAGE {
        return Err(ReadError::TooLong);
    }
    serde_json::from_slice(&line)
        .map(Some)
        .map_err(ReadError::Json)
}

/// Writes one message and flushes.
pub fn write_message<T: Serialize>(w: &mut impl Write, message: &T) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(message)?;
    bytes.push(b'\n');
    w.write_all(&bytes)?;
    w.flush()
}

/// Hex SHA-256 of a token; the app keeps only this.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The app's data folder: `TERRA_PROFILE_DIR` when set (development
/// profiles), else the platform's application data folder.
pub fn app_data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("TERRA_PROFILE_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support").join(APP_ID))
    } else {
        let data = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Some(data.join(APP_ID))
    }
}

/// Where the app's socket is: `TERRA_SOCKET` when set, else in the data
/// folder.
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("TERRA_SOCKET") {
        return Some(PathBuf::from(path));
    }
    app_data_dir().map(|d| d.join(SOCKET_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_one_per_line() {
        let mut buf = Vec::new();
        write_message(
            &mut buf,
            &Request::new(1, "hello", serde_json::json!({"token": "x"})),
        )
        .unwrap();
        write_message(
            &mut buf,
            &Response::notify("event", serde_json::json!({"name": "data-changed"})),
        )
        .unwrap();
        let mut r = std::io::Cursor::new(buf);
        let req: Request = read_message(&mut r).unwrap().unwrap();
        assert_eq!(req.method, "hello");
        let note: Response = read_message(&mut r).unwrap().unwrap();
        assert_eq!(note.method.as_deref(), Some("event"));
        assert!(note.id.is_none());
        assert!(read_message::<Request>(&mut r).unwrap().is_none());
    }

    #[test]
    fn long_and_bad_lines_are_refused() {
        let mut r = std::io::Cursor::new(vec![b'a'; MAX_MESSAGE + 10]);
        assert!(matches!(
            read_message::<Value>(&mut r),
            Err(ReadError::TooLong)
        ));
        let mut r = std::io::Cursor::new(b"{nope\n".to_vec());
        assert!(matches!(
            read_message::<Value>(&mut r),
            Err(ReadError::Json(_))
        ));
    }

    #[test]
    fn error_kinds() {
        assert_eq!(RpcError::command("forbidden", "no").kind(), "forbidden");
        assert_eq!(RpcError::protocol(PARSE_ERROR, "bad").kind(), "protocol");
        assert_eq!(token_hash("abc").len(), 64);
    }
}
