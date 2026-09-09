use alloc::string::String;

use audio_provenance_core::{CanonicalJsonError, CodedError};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("{field} must be exactly {expected} lowercase hex characters, found {found}")]
    HexLength {
        field: &'static str,
        expected: usize,
        found: usize,
    },
    #[error("{field} must contain only lowercase hexadecimal characters")]
    HexCharset { field: &'static str },
    #[error("mark {field} must fit in four bits, found {found}")]
    MarkField { field: &'static str, found: u8 },
    #[error("fingerprint must be 1..={limit} bytes, found {found}")]
    FingerprintSize { limit: usize, found: usize },
    #[error("signing time must be RFC 3339 UTC, as YYYY-MM-DDTHH:MM:SS[.fraction]Z")]
    Timestamp,
    #[error("advisory score must be finite and within 0.0..=1.0, found {found}")]
    ScoreRange { found: f32 },
    #[error("a signed manifest must be a JSON object")]
    ManifestNotObject,
    #[error("manifest carries no portable_signature block")]
    MissingSignatureBlock,
    #[error("manifest declares no usable locator_salt, so no Watermark payload can ever resolve it")]
    MissingLocatorSalt,
    #[error("locator {mark_id} is already held by record {existing}")]
    LocatorConflict { mark_id: String, existing: String },
    #[error("portable_signature block is not admissible: {reason}")]
    SignatureBlock { reason: String },
    #[error(
        "manifest does not round-trip through {}",
        audio_provenance_core::CANONICALIZATION_ID
    )]
    NoncanonicalManifest,
    #[error(transparent)]
    Canonicalization(#[from] CanonicalJsonError),
    #[error("a mark lookup result must hold 1..={limit} records, found {found}")]
    MarkMatchCount { limit: usize, found: usize },
    #[error("unsupported record format {found:?}, expected {expected:?}")]
    RecordFormat {
        found: String,
        expected: &'static str,
    },
    #[error("record {declared} does not hash to its own manifest bytes, which hash to {derived}")]
    RecordIdMismatch { declared: String, derived: String },
    #[error("malformed registry record: {reason}")]
    MalformedRecord { reason: String },
    #[error("registry path component {component:?} is not permitted")]
    PathComponent { component: String },
    #[error("registry path {relative:?} resolves outside the registry root")]
    PathEscapesRoot { relative: String },
    #[error("registry root {path:?} does not exist or is not a directory")]
    RootMissing { path: String },
    #[error(
        "registry index {path:?} is missing; run `audio-provenance registry init` against that root"
    )]
    IndexMissing { path: String },
    #[error("unsupported index format {found:?}, expected {expected:?}")]
    IndexFormat {
        found: String,
        expected: &'static str,
    },
    #[error("index lists record {record_id} more than once")]
    IndexDuplicateRecord { record_id: String },
    #[error("content hash {content_sha256} is already registered to record {existing}")]
    ContentHashConflict {
        content_sha256: String,
        existing: String,
    },
    #[error("registry base URL is unusable: {reason}")]
    BaseUrl { reason: &'static str },
    #[error("remote registry publish failed: {detail}")]
    RemotePublish { detail: String },
    #[error("{field} must be within 1..={limit}, found {found}")]
    OptionRange {
        field: &'static str,
        limit: u64,
        found: u64,
    },
    #[cfg(feature = "std")]
    #[error("registry I/O failed at {path:?}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[cfg(feature = "std")]
    #[error("registry JSON at {path:?} is malformed")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

impl CodedError for RegistryError {
    fn code(&self) -> &'static str {
        match self {
            Self::HexLength { .. } => "registry_hex_length_invalid",
            Self::HexCharset { .. } => "registry_hex_charset_invalid",
            Self::MarkField { .. } => "registry_mark_field_out_of_range",
            Self::FingerprintSize { .. } => "registry_fingerprint_size_invalid",
            Self::Timestamp => "registry_signed_at_invalid",
            Self::ScoreRange { .. } => "registry_advisory_score_out_of_range",
            Self::ManifestNotObject => "registry_manifest_not_object",
            Self::MissingSignatureBlock => "registry_manifest_signature_block_missing",
            Self::MissingLocatorSalt => "registry_record_missing_locator_salt",
            Self::LocatorConflict { .. } => "registry_locator_conflict",
            Self::SignatureBlock { .. } => "registry_manifest_signature_block_invalid",
            Self::NoncanonicalManifest => "registry_manifest_noncanonical",
            Self::Canonicalization(inner) => inner.code(),
            Self::MarkMatchCount { .. } => "registry_mark_match_count_invalid",
            Self::RecordFormat { .. } => "registry_record_format_unsupported",
            Self::RecordIdMismatch { .. } => "registry_record_id_mismatch",
            Self::MalformedRecord { .. } => "registry_record_malformed",
            Self::PathComponent { .. } => "registry_path_component_rejected",
            Self::PathEscapesRoot { .. } => "registry_path_escapes_root",
            Self::RootMissing { .. } => "registry_root_missing",
            Self::IndexMissing { .. } => "registry_index_missing",
            Self::IndexFormat { .. } => "registry_index_format_unsupported",
            Self::IndexDuplicateRecord { .. } => "registry_index_duplicate_record",
            Self::ContentHashConflict { .. } => "registry_content_hash_conflict",
            Self::BaseUrl { .. } => "registry_base_url_invalid",
            Self::RemotePublish { .. } => "registry_remote_publish_failed",
            Self::OptionRange { .. } => "registry_option_out_of_range",
            #[cfg(feature = "std")]
            Self::Io { .. } => "registry_io_failed",
            #[cfg(feature = "std")]
            Self::Json { .. } => "registry_json_malformed",
        }
    }
}

#[cfg(feature = "std")]
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("configuration file {path:?} could not be read")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration file {path:?} is not valid JSON")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("unsupported config format {found:?}, expected {expected:?}")]
    Format {
        found: String,
        expected: &'static str,
    },
    #[error("registry {name:?} declares kind {found:?}; expected \"local\" or \"http\"")]
    Kind { name: String, found: String },
    #[error("registry {name:?} is missing the required field {field:?}")]
    MissingField { name: String, field: &'static str },
    #[error(
        "registry name {name:?} is not usable; names are 1..=64 characters of a-z, 0-9, '-' and '_'"
    )]
    Name { name: String },
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

#[cfg(feature = "std")]
impl CodedError for ConfigError {
    fn code(&self) -> &'static str {
        match self {
            Self::Io { .. } => "registry_config_io_failed",
            Self::Json { .. } => "registry_config_json_malformed",
            Self::Format { .. } => "registry_config_format_unsupported",
            Self::Kind { .. } => "registry_config_kind_unsupported",
            Self::MissingField { .. } => "registry_config_field_missing",
            Self::Name { .. } => "registry_config_name_invalid",
            Self::Registry(inner) => inner.code(),
        }
    }
}

#[cfg(feature = "std")]
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// IMPORTANT: the message names the exact environment variable and config path to set.
    /// There is no published Audio Provenance registry endpoint, so a fallback URL would be a guess
    /// presented as a fact.
    #[error(
        "registry {name:?} is not configured. Set {env_var}={value_hint} in the environment, or add a {name:?} entry to {config_path:?}. No default endpoint is assumed: no public Audio Provenance registry URL is published."
    )]
    Unconfigured {
        name: String,
        env_var: &'static str,
        /// IMPORTANT: one variable names a URL and the other a directory. A single hardcoded
        /// `<base URL>` told a caller of the filesystem registry to set a root to a URL.
        value_hint: &'static str,
        config_path: String,
    },
    #[error("unknown registry {name:?}; configured registries: {known}")]
    Unknown { name: String, known: String },
    #[error("registry {name:?} is configured as HTTP, but this build has the `http` feature off")]
    HttpUnsupported { name: String },
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Registry(#[from] RegistryError),
}

#[cfg(feature = "std")]
impl CodedError for ResolveError {
    fn code(&self) -> &'static str {
        match self {
            Self::Unconfigured { .. } => "registry_not_configured",
            Self::Unknown { .. } => "registry_unknown_name",
            Self::HttpUnsupported { .. } => "registry_http_feature_disabled",
            Self::Config(inner) => inner.code(),
            Self::Registry(inner) => inner.code(),
        }
    }
}
