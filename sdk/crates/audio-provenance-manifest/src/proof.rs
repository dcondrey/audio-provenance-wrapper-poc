use alloc::string::String;

use audio_provenance_core::ProofLevel;

/// Evidential strength, weakest first.
///
/// IMPORTANT: `externally_verified` outranks `directly_observed` because it is the only level this
/// system can never self-assert. The POC observes its own capture directly and still reports
/// `unknown_unobserved` for signer identity; a level that requires an independent party is
/// therefore strictly above one this process can award itself.
pub const fn proof_rank(level: ProofLevel) -> u8 {
    match level {
        ProofLevel::UnknownUnobserved => 0,
        ProofLevel::UserDeclared => 1,
        ProofLevel::Inferred => 2,
        ProofLevel::DirectlyObserved => 3,
        ProofLevel::ExternallyVerified => 4,
    }
}

/// The LOWEST of the given levels. A composite claim can be no better evidenced than its weakest
/// input, so this is a floor and never a maximum. An empty slice has observed nothing.
pub fn cap_proof_level(levels: &[ProofLevel]) -> ProofLevel {
    levels
        .iter()
        .copied()
        .min_by_key(|level| proof_rank(*level))
        .unwrap_or(ProofLevel::UnknownUnobserved)
}

/// A claim carried at a level lower than its source asserted, with the reason recorded.
///
/// There is no way to raise a level: [`Degraded::new`] takes the floor of the requested level and
/// the claim's current one, so a caller cannot launder a weak claim into a strong one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Degraded<T> {
    claim: T,
    proof_level: ProofLevel,
    reason: String,
}

impl<T> Degraded<T> {
    pub fn new(claim: T, from: ProofLevel, to: ProofLevel, reason: impl Into<String>) -> Self {
        Self {
            claim,
            proof_level: cap_proof_level(&[from, to]),
            reason: reason.into(),
        }
    }

    pub const fn claim(&self) -> &T {
        &self.claim
    }

    pub const fn proof_level(&self) -> ProofLevel {
        self.proof_level
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn into_claim(self) -> T {
        self.claim
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degrading_upward_is_refused() {
        let d = Degraded::new(
            (),
            ProofLevel::Inferred,
            ProofLevel::ExternallyVerified,
            "a caller asked for more than the evidence carries",
        );
        assert_eq!(d.proof_level(), ProofLevel::Inferred);
    }

    #[test]
    fn cap_is_a_floor_not_a_maximum() {
        assert_eq!(
            cap_proof_level(&[
                ProofLevel::DirectlyObserved,
                ProofLevel::UnknownUnobserved,
                ProofLevel::ExternallyVerified,
            ]),
            ProofLevel::UnknownUnobserved
        );
        assert_eq!(cap_proof_level(&[]), ProofLevel::UnknownUnobserved);
    }
}
