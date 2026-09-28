//! The bridge into `apw_trace`'s verification pipeline.
//!
//! The proof level is not passed across this boundary and cannot be. `apw_trace` derives
//! `externally_verified` from a `TrustOutcome::Anchored` in its own status mapping, so the only
//! thing this adapter can do is report whether a chain reached a trusted anchor. There is no
//! setter, here or anywhere, that raises a signer's proof level.

use audio_provenance_core::KeyPossessionProof;
use apw_trace::{RevocationStatus, TrustAnchor, TrustResolution};

use crate::chain::{Refusal, TrustEvaluation};
use crate::store::TrustStore;
use crate::time::Instant;

/// A trust store bound to one evaluation instant.
///
/// The instant is fixed at construction rather than read per call, so every signer in one
/// verification run is judged against the same clock reading and this crate never reads a clock.
#[derive(Debug, Clone)]
pub struct AnchoredTrustStore {
    store: TrustStore,
    at: Instant,
}

impl AnchoredTrustStore {
    pub const fn new(store: TrustStore, at: Instant) -> Self {
        Self { store, at }
    }

    pub const fn store(&self) -> &TrustStore {
        &self.store
    }

    pub const fn evaluated_at(&self) -> &Instant {
        &self.at
    }
}

impl apw_trace::TrustStore for AnchoredTrustStore {
    fn resolve(&self, proof: &KeyPossessionProof) -> TrustResolution {
        match self.store.evaluate(proof.public_key_bytes(), &self.at) {
            TrustEvaluation::Vouched(identity) => TrustResolution::Anchored(TrustAnchor {
                identity: identity.display_name.clone(),
                authority: identity.authority(),
                revocation: if identity.revocation_list_consulted {
                    RevocationStatus::CheckedNotRevoked
                } else {
                    RevocationStatus::RevocationUnchecked
                },
            }),
            TrustEvaluation::NotCovered => TrustResolution::Unanchored,
            TrustEvaluation::Refused(refusal) => {
                let reason = format!("{}: {}", refusal.code(), refusal.describe());
                if matches!(refusal, Refusal::Revoked { .. }) {
                    TrustResolution::Revoked { reason }
                } else {
                    TrustResolution::Failed { reason }
                }
            }
        }
    }
}
