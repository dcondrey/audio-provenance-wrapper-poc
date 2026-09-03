use std::path::PathBuf;

pub type Result<T> = core::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is shorter than the bound {expected}-byte prefix")]
    ShortPrefix { path: PathBuf, expected: u64 },
    #[error("prefix byte_length {got} exceeds the {max}-byte bound")]
    PrefixTooLong { got: u64, max: u64 },
    #[error("canonical JSON rejects the non-finite number at {pointer}")]
    NonFiniteNumber { pointer: String },
    #[error("manifest nesting exceeds {max} levels at {pointer}")]
    NestingTooDeep { pointer: String, max: usize },
    #[error("invalid proof level: {value}")]
    InvalidProofLevel { value: String },
    #[error("invalid {field}: {value}")]
    InvalidEnumValue { field: &'static str, value: String },
    #[error("manifest must be a JSON object")]
    NotAnObject,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Ed25519 key material is invalid: {0}")]
    KeyMaterial(&'static str),
}

impl CoreError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        CoreError::Io {
            path: path.into(),
            source,
        }
    }
}
