use std::path::Path;

pub type Result<T> = core::result::Result<T, C2paError>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum C2paError {
    #[error("{0}")]
    UnsupportedAsset(String),

    #[error("{0}")]
    Manifest(String),

    #[error("{0}")]
    SignerSetup(String),

    #[error("{mode} signing of {name} failed: {source}")]
    Signing {
        mode: &'static str,
        name: String,
        // Boxed: c2pa::Error is ~160 bytes and would widen every Result in this crate.
        #[source]
        source: Box<c2pa::Error>,
    },

    #[error("Reading provenance from {name} failed: {source}")]
    Verification {
        name: String,
        #[source]
        source: Box<c2pa::Error>,
    },

    #[error("Trust anchors must be PEM-encoded certificates")]
    TrustAnchors,

    #[error("configuring the c2pa context failed: {0}")]
    Context(#[source] Box<c2pa::Error>),

    #[error("cannot access {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: Box<std::io::Error>,
    },

    #[error(transparent)]
    Core(#[from] apw_core::CoreError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub(crate) fn io_error(path: &Path, source: std::io::Error) -> C2paError {
    C2paError::Io {
        path: path.display().to_string(),
        source: Box::new(source),
    }
}

pub(crate) fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
