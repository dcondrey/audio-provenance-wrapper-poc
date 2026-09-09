//! Trust-chain resolution: anchored, unanchored, or failed.
//!
//! A valid self-generated signature proves KEY POSSESSION and nothing about who holds the key. That
//! is why the common case is `unanchored` with no name, and why an anchor store is a separate,
//! explicitly configured input rather than a default.

use std::collections::BTreeMap;
use std::path::Path;

use audio_provenance_core::KeyPossessionProof;
use serde::Deserialize;

use crate::error::TraceError;
use crate::status::TrustOutcome;

pub const TRUST_STORE_FORMAT: &str = "audio-provenance-trust-store-v0";

/// A resolved anchor: a name, and the authority that vouches for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustAnchor {
    pub identity: String,
    pub authority: String,
}

#[derive(Debug, Clone)]
pub enum TrustResolution {
    Anchored(TrustAnchor),
    Unanchored,
    Failed { reason: String },
}

impl TrustResolution {
    pub const fn outcome(&self) -> TrustOutcome {
        match self {
            Self::Anchored(_) => TrustOutcome::Anchored,
            Self::Unanchored => TrustOutcome::Unanchored,
            Self::Failed { .. } => TrustOutcome::Failed,
        }
    }

    pub const fn anchor(&self) -> Option<&TrustAnchor> {
        match self {
            Self::Anchored(anchor) => Some(anchor),
            Self::Unanchored | Self::Failed { .. } => None,
        }
    }
}

/// Resolves a proven key possession to a name, or declines to.
pub trait TrustStore: std::fmt::Debug + Send + Sync {
    fn resolve(&self, proof: &KeyPossessionProof) -> TrustResolution;
}

/// The default store: nothing is anchored.
///
/// Not a stub. A verifier with no configured anchors genuinely knows no identities, and saying so
/// is the correct answer; inventing one would be the bug.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTrustAnchors;

impl TrustStore for NoTrustAnchors {
    fn resolve(&self, _proof: &KeyPossessionProof) -> TrustResolution {
        TrustResolution::Unanchored
    }
}

#[derive(Debug, Deserialize)]
struct TrustStoreFile {
    format: String,
    #[serde(default)]
    anchors: Vec<AnchorEntry>,
    #[serde(default)]
    revoked: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AnchorEntry {
    signer_id: String,
    identity: String,
    authority: String,
}

/// A signer-id keyed store loaded from one JSON file.
#[derive(Debug, Clone, Default)]
pub struct FileTrustStore {
    anchors: BTreeMap<String, TrustAnchor>,
    revoked: BTreeMap<String, ()>,
}

impl FileTrustStore {
    pub fn from_json(bytes: &[u8]) -> Result<Self, TraceError> {
        let parsed: TrustStoreFile =
            serde_json::from_slice(bytes).map_err(|error| TraceError::InvalidOption {
                option: "trust_store",
                reason: error.to_string(),
            })?;
        if parsed.format != TRUST_STORE_FORMAT {
            return Err(TraceError::InvalidOption {
                option: "trust_store",
                reason: format!(
                    "expected format {TRUST_STORE_FORMAT}, found {:?}",
                    parsed.format
                ),
            });
        }
        let mut anchors = BTreeMap::new();
        for entry in parsed.anchors {
            if entry.identity.is_empty() || entry.authority.is_empty() {
                return Err(TraceError::InvalidOption {
                    option: "trust_store",
                    reason: format!("anchor {} names no identity or authority", entry.signer_id),
                });
            }
            anchors.insert(
                entry.signer_id,
                TrustAnchor {
                    identity: entry.identity,
                    authority: entry.authority,
                },
            );
        }
        let revoked = parsed
            .revoked
            .into_iter()
            .map(|signer_id| (signer_id, ()))
            .collect();
        Ok(Self { anchors, revoked })
    }

    pub fn load(path: &Path) -> Result<Self, TraceError> {
        let bytes = std::fs::read(path).map_err(|source| TraceError::Unreadable {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_json(&bytes)
    }

    pub fn len(&self) -> usize {
        self.anchors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }
}

impl TrustStore for FileTrustStore {
    fn resolve(&self, proof: &KeyPossessionProof) -> TrustResolution {
        let signer_id = proof.signer_id();
        // Revocation is checked first. An anchor that also appears in the revocation list is a
        // store that contradicts itself, and the safe reading of a contradiction is the refusal.
        if self.revoked.contains_key(signer_id) {
            return TrustResolution::Failed {
                reason: format!("signer {signer_id} is revoked"),
            };
        }
        match self.anchors.get(signer_id) {
            Some(anchor) => TrustResolution::Anchored(anchor.clone()),
            None => TrustResolution::Unanchored,
        }
    }
}
