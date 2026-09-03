use std::io;
use std::path::{Path, PathBuf};

use apw_core::CoreError;

pub type Result<T> = core::result::Result<T, DaemonError>;

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("cannot {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("cannot bind the evidence socket to {address}: {source}")]
    Bind {
        address: String,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("{context}: {source}")]
    Assembly {
        context: String,
        #[source]
        source: CoreError,
    },
}

impl DaemonError {
    pub(crate) fn io(action: &'static str, path: &Path, source: io::Error) -> DaemonError {
        DaemonError::Io {
            action,
            path: path.to_path_buf(),
            source,
        }
    }

    pub(crate) fn assembly(context: impl Into<String>, source: CoreError) -> DaemonError {
        DaemonError::Assembly {
            context: context.into(),
            source,
        }
    }
}
