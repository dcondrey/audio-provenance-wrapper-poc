use std::path::{Path, PathBuf};

pub type Result<T> = core::result::Result<T, CliError>;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("cannot {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Usage(String),

    #[error("{path}: the local manifest verifier is not part of the native engine")]
    ManifestVerifierUnported { path: PathBuf },

    #[error(transparent)]
    Core(#[from] apw_core::CoreError),

    #[error(transparent)]
    C2pa(#[from] Box<apw_c2pa::C2paError>),

    #[error(transparent)]
    Provenance(#[from] Box<apw_provenance::ProvenanceError>),

    #[error(transparent)]
    Daemon(#[from] Box<apw_daemon::DaemonError>),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CliError {
    pub fn io(action: &'static str, path: &Path, source: std::io::Error) -> CliError {
        CliError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    pub fn usage(message: impl Into<String>) -> CliError {
        CliError::Usage(message.into())
    }
}

// The three engine errors are boxed at the `From` boundary: c2pa::Error alone is
// wide enough that an unboxed variant trips clippy::result_large_err across every
// command body.
impl From<apw_c2pa::C2paError> for CliError {
    fn from(source: apw_c2pa::C2paError) -> Self {
        CliError::C2pa(Box::new(source))
    }
}

impl From<apw_provenance::ProvenanceError> for CliError {
    fn from(source: apw_provenance::ProvenanceError) -> Self {
        CliError::Provenance(Box::new(source))
    }
}

impl From<apw_daemon::DaemonError> for CliError {
    fn from(source: apw_daemon::DaemonError) -> Self {
        CliError::Daemon(Box::new(source))
    }
}

/// Collapse an error chain onto one bounded line.
///
/// IMPORTANT: this string is written into `c2pa_claim.reason` inside a signed
/// manifest, mirroring `daemon/manifest_builder/generator.py`'s
/// `" ".join(str(exc).split())[:400]`, so it must never carry a newline or grow
/// without bound.
pub fn one_line(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        // A variant whose own Display already interpolates `{source}` would
        // otherwise render its cause twice.
        let text = inner.to_string();
        if !rendered.contains(&text) {
            rendered.push_str(": ");
            rendered.push_str(&text);
        }
        source = inner.source();
    }
    let collapsed = rendered.split_whitespace().collect::<Vec<&str>>().join(" ");
    collapsed.chars().take(400).collect()
}
