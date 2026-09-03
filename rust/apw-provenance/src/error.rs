use std::path::PathBuf;

use crate::remote::RemoteRequirement;

pub type Result<T> = core::result::Result<T, ProvenanceError>;

/// Signing refused because the key is revoked.
///
/// IMPORTANT: this is refusal to mint a NEW signature. Prior signing records are
/// retained; see `ProvenanceProvider::revoke`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("key {key_id} is revoked and cannot {attempted}")]
pub struct RevokedKeyError {
    pub key_id: String,
    pub attempted: &'static str,
}

impl RevokedKeyError {
    pub const ISSUE_MATERIAL: &'static str = "issue signing material";
    pub const SIGN_CLAIM: &'static str = "sign new claims";
}

#[derive(Debug, thiserror::Error)]
pub enum ProvenanceError {
    #[error(transparent)]
    RevokedKey(#[from] RevokedKeyError),

    #[error("cannot {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(transparent)]
    Core(#[from] apw_core::CoreError),

    #[error("{path} is not valid JSON: {source}")]
    Record {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("unknown key_id: {key_id}")]
    UnknownKey { key_id: String },

    #[error("certificate issuance failed: {0}")]
    Issuance(String),

    #[error("key material is invalid: {0}")]
    KeyMaterial(String),

    #[error(transparent)]
    Descriptor(#[from] crate::mark::DescriptorError),

    #[error("manifest must carry a 64-character content_sha256 for registration")]
    MissingContentDigest,

    #[error("{0}")]
    Seal(&'static str),

    #[error("asset is too short to describe: {path}")]
    AssetTooShort { path: PathBuf },

    #[error("no remote provenance service exists.\n{0}")]
    RemoteServiceMissing(&'static RemoteRequirement),
}

impl ProvenanceError {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        ProvenanceError::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

impl From<rcgen::Error> for ProvenanceError {
    fn from(value: rcgen::Error) -> Self {
        ProvenanceError::Issuance(value.to_string())
    }
}
