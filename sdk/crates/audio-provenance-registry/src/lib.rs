//! The Audio Provenance registry: signed manifests, queryable by Watermark id, by exact content hash and
//! by perceptual fingerprint.
//!
//! # A miss and an outage are different answers
//!
//! [`Lookup`] has three arms, and there is no conversion to `Option`. A backend that cannot be
//! reached returns [`Lookup::Unavailable`], never [`Lookup::NotFound`], because collapsing the
//! two turns a network fault into the provenance verdict "unregistered".
//!
//! # There is no default remote
//!
//! No public Audio Provenance registry endpoint is published, so this crate hardcodes no host, no path
//! prefix. [`HttpRegistryBackend`] takes a base URL; its authenticated constructor adds bearer or
//! Cloudflare Access service-token headers and enables idempotent record publication. The
//! `public` name resolves through `AUDIO_PROVENANCE_REGISTRY_URL` or a configuration file.
//! Unconfigured is an error that names what to set.
//!
//! # Offline is the default, not the fallback
//!
//! [`LocalRegistryBackend`] is a directory. Nothing in it touches a network.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

extern crate alloc;

pub mod backend;
pub mod error;
pub mod ids;
pub mod lookup;
pub mod record;

#[cfg(feature = "std")]
pub mod config;
#[cfg(feature = "http")]
pub mod http;
#[cfg(feature = "local")]
pub mod local;
#[cfg(feature = "local")]
pub mod resolver;

pub use backend::{RegistryBackend, RegistryKind, RegistrySource, WritableRegistryBackend};
pub use error::RegistryError;
pub use ids::{
    ContentHash, DIGEST_BYTES, Fingerprint, LOCATOR_BYTES, MARK_ID_HEX_LEN, MAX_FINGERPRINT_BYTES,
    MarkId, RecordId, SignedAt,
};
pub use lookup::{Lookup, Unavailable, UnavailableKind};
pub use record::{
    AdvisoryScore, MAX_FINGERPRINT_CANDIDATES, MAX_MARK_MATCHES, MarkMatches, RECORD_FORMAT,
    RegistryRecord, ScoredCandidate,
};

#[cfg(feature = "std")]
pub use config::{
    CONFIG_FILE_NAME, CONFIG_FORMAT, EnvSource, HttpEntry, MapEnv, ProcessEnv, REGISTRY_ROOT_ENV,
    REGISTRY_URL_ENV, RegistryConfig, RegistryEntry, load_config,
};
#[cfg(feature = "std")]
pub use error::{ConfigError, ResolveError};
#[cfg(feature = "http")]
pub use http::{HttpRegistryAuth, HttpRegistryBackend, HttpRegistryOptions, PROTOCOL_ID};
#[cfg(feature = "local")]
pub use local::LocalRegistryBackend;
#[cfg(feature = "local")]
pub use resolver::{LOCAL_REGISTRY, PUBLIC_REGISTRY, RegistryResolver};
