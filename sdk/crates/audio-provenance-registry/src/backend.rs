use alloc::string::String;
use core::fmt;

use crate::ids::{ContentHash, Fingerprint, MarkId, RecordId};
use crate::lookup::Lookup;
use crate::record::{MarkMatches, RegistryRecord, ScoredCandidate};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegistryKind {
    Local,
    Http,
    /// Records the caller handed over outright, held in memory. The wasm build has no filesystem
    /// and no socket, so this is the only backend it can offer; naming it `filesystem` would put a
    /// false sentence in every result the browser prints.
    Memory,
}

impl RegistryKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "filesystem",
            Self::Http => "http",
            Self::Memory => "memory",
        }
    }
}

impl fmt::Display for RegistryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a verifier prints when it says which registry answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySource {
    name: String,
    kind: RegistryKind,
    location: String,
}

impl RegistrySource {
    pub fn new(name: impl Into<String>, kind: RegistryKind, location: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            location: location.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn kind(&self) -> RegistryKind {
        self.kind
    }

    /// The root directory or base URL. Credentials are rejected at construction, so this is
    /// always safe to print.
    pub fn location(&self) -> &str {
        &self.location
    }
}

/// A queryable repository of signed manifests.
///
/// Synchronous by choice: every consumer (the CLI, and GenoTrace's recovery ladder) issues
/// strictly sequential lookups, and a sync trait is dyn-compatible without boxing every call.
pub trait RegistryBackend: fmt::Debug + Send + Sync {
    fn source(&self) -> &RegistrySource;

    /// Every record sharing the mark's 48-bit locator, because the locator is an index and not
    /// an identity.
    fn lookup_by_mark(&self, mark: &MarkId) -> Lookup<MarkMatches>;

    fn lookup_by_content_hash(&self, content_hash: &ContentHash) -> Lookup<RegistryRecord>;

    /// Nearest neighbours by perceptual similarity, most similar first. The scores are
    /// advisory; the caller re-derives them before letting one influence a verdict.
    fn nearest_by_fingerprint(
        &self,
        query: &Fingerprint,
        limit: usize,
    ) -> Lookup<alloc::vec::Vec<ScoredCandidate>>;

    fn fetch(&self, record_id: &RecordId) -> Lookup<RegistryRecord>;
}

/// A registry that can atomically accept an already-signed record.
///
/// Kept separate from [`RegistryBackend`] so read-only resolvers and browser-provided record sets
/// cannot accidentally be used as publication targets.
pub trait WritableRegistryBackend: RegistryBackend {
    fn put(&self, record: &RegistryRecord) -> Result<(), crate::RegistryError>;
}
