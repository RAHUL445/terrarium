//! Plugin SDK. Implement [`Plugin`] and register it with [`plugin!`]:
//!
//! ```ignore
//! struct Todo;
//! impl terrarium_guest::Plugin for Todo {
//!     fn handle(op: &str, args: terrarium_guest::Args) -> terrarium_guest::Result<serde_json::Value> {
//!         match op {
//!             "create" => { /* ... */ }
//!             _ => Err(terrarium_guest::Error::unknown_operation(op)),
//!         }
//!     }
//! }
//! terrarium_guest::plugin!(Todo);
//! ```

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        world: "plugin",
        path: "../../wit",
        pub_export_macro: true,
        export_macro_name: "__export_world",
        default_bindings_module: "terrarium_guest::bindings",
    });
}

use bindings::terrarium::plugin::{
    ai as host_ai, clock as host_clock, interop as host_interop, log as host_log,
    schedule as host_schedule, speech as host_speech, storage as host,
};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct Error {
    pub code: String,
    pub message: String,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Error {
            code: code.to_string(),
            message: message.into(),
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Error::new("invalid", message)
    }
    pub fn unknown_operation(op: &str) -> Self {
        Error::new("unknown-operation", format!("unknown operation `{op}`"))
    }
}

impl From<host::PluginError> for Error {
    fn from(e: host::PluginError) -> Self {
        Error {
            code: e.code,
            message: e.message,
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::invalid(e.to_string())
    }
}

/// Operation arguments with typed accessors.
pub struct Args(pub Map<String, Value>);

impl Args {
    pub fn str(&self, key: &str) -> Result<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::invalid(format!("`{key}` must be text")))
    }
    pub fn opt_str(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(Value::as_str)
    }
    pub fn bool(&self, key: &str) -> Result<bool> {
        self.0
            .get(key)
            .and_then(Value::as_bool)
            .ok_or_else(|| Error::invalid(format!("`{key}` must be true or false")))
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }
}

pub trait Plugin {
    fn handle(operation: &str, args: Args) -> Result<Value>;
}

/// Host storage: the plugin's own database.
pub mod storage {
    use super::*;

    pub fn query(sql: &str, params: &[Value]) -> Result<Vec<Map<String, Value>>> {
        let rows = host::query(sql, &serde_json::to_string(params)?)?;
        Ok(serde_json::from_str(&rows)?)
    }

    pub fn query_as<T: DeserializeOwned>(sql: &str, params: &[Value]) -> Result<Vec<T>> {
        let rows = host::query(sql, &serde_json::to_string(params)?)?;
        Ok(serde_json::from_str(&rows)?)
    }

    pub fn insert(table: &str, values: &Map<String, Value>) -> Result<String> {
        Ok(host::insert(table, &serde_json::to_string(values)?)?)
    }

    pub fn update(table: &str, row: &str, values: &Map<String, Value>) -> Result<()> {
        Ok(host::update(table, row, &serde_json::to_string(values)?)?)
    }

    pub fn increment(table: &str, row: &str, column: &str, delta: Value) -> Result<()> {
        Ok(host::increment(table, row, column, &delta.to_string())?)
    }

    pub fn delete(table: &str, row: &str) -> Result<()> {
        Ok(host::delete(table, row)?)
    }

    pub fn restore(table: &str, row: &str) -> Result<()> {
        Ok(host::restore(table, row)?)
    }
}

/// Time and calendar. `tz` of `""` means the user's timezone.
pub mod clock {
    use super::*;

    pub fn now_ms() -> u64 {
        host_clock::now_ms()
    }
    pub fn timezone() -> String {
        host_clock::timezone()
    }
    /// Today's local date, `YYYY-MM-DD`.
    pub fn today(tz: &str) -> Result<String> {
        Ok(host_clock::today(tz)?)
    }
    pub fn local_date(at_ms: u64, tz: &str) -> Result<String> {
        Ok(host_clock::local_date(at_ms, tz)?)
    }
    /// UTC ms for a local `YYYY-MM-DD` date and `HH:MM` time.
    pub fn local_to_utc(date: &str, time: &str, tz: &str) -> Result<u64> {
        Ok(host_clock::local_to_utc(date, time, tz)?)
    }
    /// Monday = 0 … Sunday = 6.
    pub fn weekday(date: &str) -> Result<u8> {
        Ok(host_clock::weekday(date)?)
    }
    pub fn add_days(date: &str, days: i32) -> Result<String> {
        Ok(host_clock::add_days(date, days)?)
    }
}

/// Calls to other plugins through interfaces listed under `uses`.
pub mod interop {
    use super::*;

    pub fn call(iface: &str, operation: &str, args: &Value) -> Result<Value> {
        let reply = host_interop::call(iface, operation, &args.to_string())?;
        Ok(serde_json::from_str(&reply)?)
    }

    pub fn available(iface: &str) -> bool {
        host_interop::available(iface)
    }
}

/// Local speech-to-text, on devices that have it (see `available`).
pub mod speech {
    use super::*;

    pub fn available() -> bool {
        host_speech::available()
    }

    pub fn transcribe(audio_base64: &str) -> Result<String> {
        Ok(host_speech::transcribe(audio_base64)?)
    }
}

/// Text generation through the user's AI provider (needs the `ai`
/// permission). Replies arrive later, at the operation named `on_done`.
pub mod ai {
    use super::*;

    pub fn available() -> bool {
        host_ai::available()
    }

    /// `request`: `{system?, prompt, model?, max_tokens?, temperature?, context?}`.
    /// Returns the request id.
    pub fn submit(request: &Value, on_done: &str) -> Result<String> {
        Ok(host_ai::submit(&request.to_string(), on_done)?)
    }
}

/// The plugin's own schedules, run by the host.
pub mod schedule {
    use super::*;

    pub fn set(id: &str, schedule: &Value) -> Result<()> {
        Ok(host_schedule::set(id, &schedule.to_string())?)
    }

    pub fn remove(id: &str) -> Result<()> {
        Ok(host_schedule::remove(id)?)
    }

    pub fn list() -> Result<Vec<Value>> {
        let json = host_schedule::list()?;
        Ok(serde_json::from_str(&json)?)
    }

    pub fn runs(id: &str, limit: u32) -> Result<Vec<Value>> {
        let json = host_schedule::runs(id, limit)?;
        Ok(serde_json::from_str(&json)?)
    }

    pub fn run_now(id: &str) -> Result<String> {
        Ok(host_schedule::run_now(id)?)
    }

    pub fn retry(run_id: &str) -> Result<()> {
        Ok(host_schedule::retry(run_id)?)
    }
}

pub fn log(message: &str) {
    host_log::info(message)
}

/// Glue between the generated world export and a [`Plugin`] implementation.
#[doc(hidden)]
pub fn __dispatch<P: Plugin>(
    operation: String,
    args: String,
) -> std::result::Result<String, host::PluginError> {
    let to_wit = |e: Error| host::PluginError {
        code: e.code,
        message: e.message,
    };
    let args: Map<String, Value> = match serde_json::from_str::<Value>(&args) {
        Ok(Value::Object(map)) => map,
        Ok(Value::Null) => Map::new(),
        Ok(_) => return Err(to_wit(Error::invalid("arguments must be a JSON object"))),
        Err(e) => return Err(to_wit(e.into())),
    };
    P::handle(&operation, Args(args))
        .map(|v| v.to_string())
        .map_err(to_wit)
}

/// Registers a [`Plugin`] type as this component's export.
#[macro_export]
macro_rules! plugin {
    ($ty:ident) => {
        struct __TerrariumExport;
        impl $crate::bindings::Guest for __TerrariumExport {
            fn handle(
                operation: ::std::string::String,
                args: ::std::string::String,
            ) -> ::std::result::Result<::std::string::String, $crate::bindings::terrarium::plugin::types::PluginError> {
                $crate::__dispatch::<$ty>(operation, args)
            }
        }
        $crate::bindings::__export_world!(__TerrariumExport with_types_in $crate::bindings);
    };
}
