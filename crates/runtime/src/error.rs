use thiserror::Error;

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("{path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
    #[error("build failed: {0}")]
    Build(String),
    #[error("wasm: {0}")]
    Wasm(String),
    #[error("package: {0}")]
    Package(String),
    #[error("theme: {0}")]
    Theme(String),
    #[error(transparent)]
    Core(#[from] terrarium_core::Error),
    /// The plugin returned an error from its operation.
    #[error("{message}")]
    Plugin { code: String, message: String },
    /// The plugin was stopped: out of fuel, timed out, out of memory, or crashed.
    #[error("plugin stopped: {0}")]
    Trap(String),
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    #[error("no open space `{0}`")]
    UnknownSpace(String),
    #[error("plugin `{plugin}` has no {kind} `{name}`")]
    Undeclared {
        plugin: String,
        kind: &'static str,
        name: String,
    },
}

impl RuntimeError {
    pub(crate) fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        RuntimeError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

impl From<wasmtime::Error> for RuntimeError {
    fn from(e: wasmtime::Error) -> Self {
        RuntimeError::Wasm(format!("{e:#}"))
    }
}

impl From<serde_json::Error> for RuntimeError {
    fn from(e: serde_json::Error) -> Self {
        RuntimeError::Package(e.to_string())
    }
}
