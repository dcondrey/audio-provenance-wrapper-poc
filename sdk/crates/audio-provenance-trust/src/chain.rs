//! Chain evaluation: the only thing in this system that turns a proven key into a name.
//!
//! THE RULE. A chain answers one question: does this key resolve to a name AT the evaluation
//! instant the caller supplied? Every time-based test, anchor window, record window and revocation
//! alike, is applied against that one instant. No test consumes the signer-asserted `signed_at`
//! from the manifest, because a key thief sets that field for free; a rule of the form "signatures
//! predating revocation still resolve" would hand the identity straight back to them. Revocation is
//! therefore retroactive, and so is expiry, and [`Refusal::describe`] says so in the output rather
//! than leaving it to be inferred.
//!
//! Nothing here reads a clock.

use std::collections::BTreeSet;

use crate::document::{Capability, RevocationReason, SignedAnchor, SignedRecord};
use crate::store::TrustStore;
use crate::time::Instant;

/// A name, and the anchor that vouched for it.
///
/// The anchor is not decoration. A self-signed anchor the operator chose to trust is still
/// `externally_verified` relative to THAT anchor, so the consumer needs the anchor's id and key to
/// judge it rather than take the name on faith.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VouchedIdentity {
    pub display_name: String,
    pub record_id: String,
    pub anchor_id: String,
    pub anchor_name: String,
    pub anchor_key_id: String,
    pub chain_depth: u64,
}

impl VouchedIdentity {
    /// The authority string a verification result carries beside the name.
    pub fn authority(&self) -> String {
        format!(
            "{} [anchor {} key {} depth {}]",
            self.anchor_name, self.anchor_id, self.anchor_key_id, self.chain_depth
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Revoked {
        display_name: String,
        anchor_id: String,
        anchor_key_id: String,
        revoked_at: Instant,
        reason: RevocationReason,
    },
    RecordNotYetValid {
        record_id: String,
        not_before: Instant,
    },
    RecordExpired {
        record_id: String,
        not_after: Instant,
    },
    AnchorNotYetValid {
        anchor_id: String,
        not_before: Instant,
    },
    AnchorExpired {
        anchor_id: String,
        not_after: Instant,
    },
    /// The chain is longer than the ceiling, or longer than the anchor's own policy allows.
    ChainTooDeep {
        limit: u64,
    },
    ChainCycle,
    IssuerUnknown {
        record_id: String,
        issuer_key_id: String,
    },
    /// The issuer key belongs to a leaf record, which may be named but may not vouch.
    IssuerNotPermitted {
        record_id: String,
        issuer_key_id: String,
    },
    /// A link names one anchor while its issuer chains to another.
    IssuerAnchorMismatch {
        record_id: String,
        declared: String,
        found: String,
    },
    RecordSignatureInvalid {
        record_id: String,
    },
    /// The store covers this key, but only as an issuing authority. An issuer is never resolved to
    /// a displayed identity.
    SubjectIsIssuerOnly {
        record_id: String,
    },
}

impl Refusal {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Revoked { .. } => "signer_revoked",
            Self::RecordNotYetValid { .. } => "signer_record_not_yet_valid",
            Self::RecordExpired { .. } => "signer_record_expired",
            Self::AnchorNotYetValid { .. } => "trust_anchor_not_yet_valid",
            Self::AnchorExpired { .. } => "trust_anchor_expired",
            Self::ChainTooDeep { .. } => "trust_chain_too_deep",
            Self::ChainCycle => "trust_chain_cycle",
            Self::IssuerUnknown { .. } => "trust_chain_issuer_unknown",
            Self::IssuerNotPermitted { .. } => "trust_chain_issuer_not_permitted",
            Self::IssuerAnchorMismatch { .. } => "trust_chain_issuer_anchor_mismatch",
            Self::RecordSignatureInvalid { .. } => "signer_record_signature_invalid",
            Self::SubjectIsIssuerOnly { .. } => "signer_record_is_issuer_only",
        }
    }

    /// The sentence a verifier prints. Every time-based refusal states that it is evaluated at the
    /// verification instant, so "why does a signature from last year fail now" is answered in the
    /// output rather than in a specification the reader does not have.
    pub fn describe(&self) -> String {
        match self {
            Self::Revoked {
                display_name,
                anchor_id,
                anchor_key_id,
                revoked_at,
                reason,
            } => format!(
                "{display_name}, vouched for by anchor {anchor_id} (key {anchor_key_id}), was \
                 revoked at {revoked_at} for {}. Revocation is retroactive: this signer's \
                 signatures are refused an identity regardless of when they claim to have been \
                 made, because a claimed signing time is asserted by the signer. The record and \
                 the revocation both stay in the store and stay inspectable.",
                reason.as_str()
            ),
            Self::RecordNotYetValid {
                record_id,
                not_before,
            } => format!(
                "signer record {record_id} is not valid until {not_before}, evaluated at the \
                 verification instant"
            ),
            Self::RecordExpired {
                record_id,
                not_after,
            } => format!(
                "signer record {record_id} expired at {not_after}; expiry, like revocation, is \
                 evaluated at the verification instant and is not waived for an earlier claimed \
                 signing time"
            ),
            Self::AnchorNotYetValid {
                anchor_id,
                not_before,
            } => format!("trust anchor {anchor_id} is not valid until {not_before}"),
            Self::AnchorExpired {
                anchor_id,
                not_after,
            } => format!("trust anchor {anchor_id} expired at {not_after}"),
            Self::ChainTooDeep { limit } => {
                format!("the chain to a trust anchor is longer than the {limit} links permitted")
            }
            Self::ChainCycle => "the issuer chain returns to a key it already passed".to_string(),
            Self::IssuerUnknown {
                record_id,
                issuer_key_id,
            } => format!(
                "signer record {record_id} names issuer key {issuer_key_id}, which is neither a \
                 trust anchor nor an issuing record in this store"
            ),
            Self::IssuerNotPermitted {
                record_id,
                issuer_key_id,
            } => format!(
                "signer record {record_id} was issued by key {issuer_key_id}, which holds a leaf \
                 record and is not permitted to vouch for others"
            ),
            Self::IssuerAnchorMismatch {
                record_id,
                declared,
                found,
            } => format!(
                "signer record {record_id} names anchor {declared} but its issuer chains to \
                 {found}"
            ),
            Self::RecordSignatureInvalid { record_id } => format!(
                "the signature on signer record {record_id} does not verify under its issuer's key"
            ),
            Self::SubjectIsIssuerOnly { record_id } => format!(
                "record {record_id} makes this key an issuing authority, which is never resolved \
                 to a displayed identity"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustEvaluation {
    /// Chained to a trusted anchor. This, and only this, is `externally_verified`.
    Vouched(VouchedIdentity),
    /// No record in the store covers this key. Not a failure: a store that knows nothing about a
    /// signer must say nothing about them.
    NotCovered,
    /// The store covers this key and declines to name it.
    Refused(Refusal),
}

impl TrustEvaluation {
    pub const fn identity(&self) -> Option<&VouchedIdentity> {
        match self {
            Self::Vouched(identity) => Some(identity),
            Self::NotCovered | Self::Refused(_) => None,
        }
    }
}

struct Chain<'a> {
    anchor: &'a SignedAnchor,
    /// Leaf first, then each issuing record up to the anchor.
    records: Vec<&'a SignedRecord>,
}

impl TrustStore {
    /// Resolves `subject_public_key` to a name at `at`, or says why it will not.
    pub fn evaluate(&self, subject_public_key: &[u8; 32], at: &Instant) -> TrustEvaluation {
        let candidates = self.records_for(subject_public_key);
        if candidates.is_empty() {
            return TrustEvaluation::NotCovered;
        }

        let mut refusal: Option<Refusal> = None;
        // Records are visited in a stable order and the first success wins, so a store holding a
        // valid record beside an expired one resolves rather than flipping on map iteration order.
        for candidate in candidates {
            let outcome = self.evaluate_candidate(candidate, at);
            match outcome {
                TrustEvaluation::Vouched(identity) => return TrustEvaluation::Vouched(identity),
                TrustEvaluation::Refused(found) => {
                    if refusal
                        .as_ref()
                        .is_none_or(|held| severity(&found) > severity(held))
                    {
                        refusal = Some(found);
                    }
                }
                TrustEvaluation::NotCovered => {}
            }
        }
        refusal.map_or(TrustEvaluation::NotCovered, TrustEvaluation::Refused)
    }

    fn evaluate_candidate(&self, leaf: &SignedRecord, at: &Instant) -> TrustEvaluation {
        if leaf.record().capability == Capability::Issuer {
            return TrustEvaluation::Refused(Refusal::SubjectIsIssuerOnly {
                record_id: leaf.record().record_id.clone(),
            });
        }
        let chain = match self.build_chain(leaf) {
            Ok(chain) => chain,
            Err(refusal) => return TrustEvaluation::Refused(refusal),
        };
        let anchor = chain.anchor.anchor();
        let depth = chain.records.len() as u64;
        if depth > anchor.max_chain_depth {
            return TrustEvaluation::Refused(Refusal::ChainTooDeep {
                limit: anchor.max_chain_depth,
            });
        }
        // Revocation outranks expiry: a revoked signer whose record has also lapsed should be
        // reported as revoked, which is the statement that matters to whoever reads it.
        if let Some(refusal) = self.revocation_for(&chain, leaf) {
            return TrustEvaluation::Refused(refusal);
        }
        if !anchor.window.contains(at) {
            return TrustEvaluation::Refused(if *at < anchor.window.not_before {
                Refusal::AnchorNotYetValid {
                    anchor_id: anchor.anchor_id.clone(),
                    not_before: anchor.window.not_before.clone(),
                }
            } else {
                Refusal::AnchorExpired {
                    anchor_id: anchor.anchor_id.clone(),
                    not_after: anchor.window.not_after.clone(),
                }
            });
        }
        for link in &chain.records {
            let record = link.record();
            if !record.window.contains(at) {
                return TrustEvaluation::Refused(if *at < record.window.not_before {
                    Refusal::RecordNotYetValid {
                        record_id: record.record_id.clone(),
                        not_before: record.window.not_before.clone(),
                    }
                } else {
                    Refusal::RecordExpired {
                        record_id: record.record_id.clone(),
                        not_after: record.window.not_after.clone(),
                    }
                });
            }
        }
        TrustEvaluation::Vouched(VouchedIdentity {
            display_name: leaf.record().display_name.clone(),
            record_id: leaf.record().record_id.clone(),
            anchor_id: anchor.anchor_id.clone(),
            anchor_name: anchor.name.clone(),
            anchor_key_id: anchor.key_id(),
            chain_depth: depth,
        })
    }

    /// Walks leaf to anchor, verifying each link's signature under the key that issued it. The
    /// depth ceiling is checked before each step, so an adversarial store cannot make one lookup
    /// walk the whole record set.
    fn build_chain<'a>(&'a self, leaf: &'a SignedRecord) -> Result<Chain<'a>, Refusal> {
        let mut records: Vec<&SignedRecord> = Vec::new();
        let mut visited: BTreeSet<[u8; 32]> = BTreeSet::new();
        let mut current = leaf;
        visited.insert(current.record().subject_public_key);

        loop {
            if records.len() as u64 >= crate::document::CHAIN_DEPTH_CEILING {
                return Err(Refusal::ChainTooDeep {
                    limit: crate::document::CHAIN_DEPTH_CEILING,
                });
            }
            let record = current.record();
            if let Some(anchor) = self.anchor_by_key(&record.issuer_public_key) {
                if anchor.anchor().anchor_id != record.issuer_anchor_id {
                    return Err(Refusal::IssuerAnchorMismatch {
                        record_id: record.record_id.clone(),
                        declared: record.issuer_anchor_id.clone(),
                        found: anchor.anchor().anchor_id.clone(),
                    });
                }
                verify_link(current, &anchor.anchor().public_key)?;
                records.push(current);
                return Ok(Chain { anchor, records });
            }

            let issuer = self
                .records_for(&record.issuer_public_key)
                .iter()
                .find(|candidate| candidate.record().capability == Capability::Issuer);
            let Some(issuer) = issuer else {
                let key_id = audio_provenance_core::signing::signer_id_for_public_key(
                    &record.issuer_public_key,
                );
                return Err(if self.records_for(&record.issuer_public_key).is_empty() {
                    Refusal::IssuerUnknown {
                        record_id: record.record_id.clone(),
                        issuer_key_id: key_id,
                    }
                } else {
                    Refusal::IssuerNotPermitted {
                        record_id: record.record_id.clone(),
                        issuer_key_id: key_id,
                    }
                });
            };
            if issuer.record().issuer_anchor_id != record.issuer_anchor_id {
                return Err(Refusal::IssuerAnchorMismatch {
                    record_id: record.record_id.clone(),
                    declared: record.issuer_anchor_id.clone(),
                    found: issuer.record().issuer_anchor_id.clone(),
                });
            }
            verify_link(current, &issuer.record().subject_public_key)?;
            if !visited.insert(issuer.record().subject_public_key) {
                return Err(Refusal::ChainCycle);
            }
            records.push(current);
            current = issuer;
        }
    }

    /// Only the anchor's own signed list revokes, and only for keys on the chain that ends at that
    /// anchor. A list signed by anyone else cannot take a name away.
    fn revocation_for(&self, chain: &Chain<'_>, leaf: &SignedRecord) -> Option<Refusal> {
        let anchor = chain.anchor.anchor();
        for signed in self.revocation_lists() {
            let list = signed.list();
            if list.anchor_id != anchor.anchor_id || list.issuer_public_key != anchor.public_key {
                continue;
            }
            for entry in &list.entries {
                if chain
                    .records
                    .iter()
                    .any(|link| link.record().subject_public_key == entry.subject_public_key)
                {
                    return Some(Refusal::Revoked {
                        display_name: leaf.record().display_name.clone(),
                        anchor_id: anchor.anchor_id.clone(),
                        anchor_key_id: anchor.key_id(),
                        revoked_at: entry.revoked_at.clone(),
                        reason: entry.reason,
                    });
                }
            }
        }
        None
    }
}

fn verify_link(record: &SignedRecord, issuer_public_key: &[u8; 32]) -> Result<(), Refusal> {
    record
        .verify_under(issuer_public_key)
        .map_err(|_| Refusal::RecordSignatureInvalid {
            record_id: record.record().record_id.clone(),
        })
}

/// Which refusal to report when several records for one key each decline. A revocation is the most
/// consequential statement a store makes about a signer, so it is never hidden behind a structural
/// complaint about a different record.
const fn severity(refusal: &Refusal) -> u8 {
    match refusal {
        Refusal::Revoked { .. } => 5,
        Refusal::RecordExpired { .. } | Refusal::AnchorExpired { .. } => 4,
        Refusal::RecordNotYetValid { .. } | Refusal::AnchorNotYetValid { .. } => 3,
        Refusal::RecordSignatureInvalid { .. } => 2,
        Refusal::SubjectIsIssuerOnly { .. } => 1,
        Refusal::ChainTooDeep { .. }
        | Refusal::ChainCycle
        | Refusal::IssuerUnknown { .. }
        | Refusal::IssuerNotPermitted { .. }
        | Refusal::IssuerAnchorMismatch { .. } => 0,
    }
}
