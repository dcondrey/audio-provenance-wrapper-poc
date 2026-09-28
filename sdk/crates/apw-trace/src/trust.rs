//! Trust-chain resolution: anchored, unanchored, or failed.
//!
//! A valid self-generated signature proves KEY POSSESSION and nothing about who holds the key. That
//! is why the common case is `unanchored` with no name, and why an anchor store is a separate,
//! explicitly configured input rather than a default.

use std::collections::BTreeMap;
use std::path::Path;

use audio_provenance_core::KeyPossessionProof;
use serde::{Deserialize, Serialize};

use crate::error::TraceError;
use crate::status::TrustOutcome;

pub const TRUST_STORE_FORMAT: &str = "audio-provenance-trust-store-v0";

/// What is known about revocation of the signing key behind a resolved identity.
///
/// Four-way on purpose: "not revoked" and "no revocation list was available to ask" are different
/// statements, and folding the second into the first would let a verifier with no list report a
/// clean bill of health it never checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RevocationStatus {
    /// No identity was resolved, so there is nothing to revoke.
    #[default]
    NotApplicable,
    /// A revocation list published by the vouching anchor was consulted and does not list the key.
    CheckedNotRevoked,
    /// The vouching authority revoked the key. The verdict fails closed.
    Revoked,
    /// An identity was resolved but no revocation list from its anchor was available. This is not
    /// evidence of non-revocation.
    RevocationUnchecked,
}

impl RevocationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::CheckedNotRevoked => "checked_not_revoked",
            Self::Revoked => "revoked",
            Self::RevocationUnchecked => "revocation_unchecked",
        }
    }
}

/// A resolved anchor: a name, the authority that vouches for it, and whether revocation was checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustAnchor {
    pub identity: String,
    pub authority: String,
    /// Only [`RevocationStatus::CheckedNotRevoked`] or [`RevocationStatus::RevocationUnchecked`].
    pub revocation: RevocationStatus,
}

#[derive(Debug, Clone)]
pub enum TrustResolution {
    Anchored(TrustAnchor),
    Unanchored,
    Failed { reason: String },
    /// The vouching authority revoked this signer. Verdict-wise identical to `Failed`.
    Revoked { reason: String },
}

impl TrustResolution {
    pub const fn outcome(&self) -> TrustOutcome {
        match self {
            Self::Anchored(_) => TrustOutcome::Anchored,
            Self::Unanchored => TrustOutcome::Unanchored,
            Self::Failed { .. } | Self::Revoked { .. } => TrustOutcome::Failed,
        }
    }

    pub const fn revocation(&self) -> RevocationStatus {
        match self {
            Self::Anchored(anchor) => anchor.revocation,
            Self::Revoked { .. } => RevocationStatus::Revoked,
            Self::Unanchored | Self::Failed { .. } => RevocationStatus::NotApplicable,
        }
    }

    pub const fn anchor(&self) -> Option<&TrustAnchor> {
        match self {
            Self::Anchored(anchor) => Some(anchor),
            Self::Unanchored | Self::Failed { .. } | Self::Revoked { .. } => None,
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
    /// `None` when the document declares no revocation list at all. This store is unsigned, so even
    /// a declared list is only the operator's assertion; see `resolve`.
    #[serde(default)]
    revoked: Option<Vec<String>>,
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
    revocation_declared: bool,
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
                    revocation: RevocationStatus::RevocationUnchecked,
                },
            );
        }
        let revocation_declared = parsed.revoked.is_some();
        let revoked = parsed
            .revoked
            .unwrap_or_default()
            .into_iter()
            .map(|signer_id| (signer_id, ()))
            .collect();
        Ok(Self {
            anchors,
            revoked,
            revocation_declared,
        })
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
            return TrustResolution::Revoked {
                reason: format!("signer {signer_id} is revoked"),
            };
        }
        match self.anchors.get(signer_id) {
            Some(anchor) => {
                let mut anchor = anchor.clone();
                // A flat store's list is unsigned and names no issuer, so it is credited as a
                // check only when the operator declared one.
                anchor.revocation = if self.revocation_declared {
                    RevocationStatus::CheckedNotRevoked
                } else {
                    RevocationStatus::RevocationUnchecked
                };
                TrustResolution::Anchored(anchor)
            }
            None => TrustResolution::Unanchored,
        }
    }
}
