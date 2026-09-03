//! What `verify` returns, and the evidence-carrying types that reduce onto [`crate::status`].

use audio_provenance_core::{ProofLevel, VerificationStatus};
use audio_provenance_manifest::{Finding, Manifest};
use audio_provenance_registry::RegistrySource;
use serde::Serialize;

use crate::nulltest::FalsePositiveRate;
use crate::status::{BindingClass, Verdict};

/// Default coverage floor for a soft-only `verified`, the POC's `alignment_threshold`.
pub const DEFAULT_SOFT_BINDING_THRESHOLD: f64 = 0.72;

/// What produced `match`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchBasis {
    HardExact,
    Genomark,
    Fingerprint,
    /// A CRC-valid GenoMark payload corroborated by the record's own signed reference
    /// constellation. Distinct from [`Self::Fingerprint`] on purpose: a fingerprint alone never
    /// verifies, and collapsing the two would make that invariant unreadable from the result.
    MarkAndFingerprint,
    None,
}

impl MatchBasis {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HardExact => "hard_exact",
            Self::Genomark => "genomark",
            Self::Fingerprint => "fingerprint",
            Self::MarkAndFingerprint => "mark_and_fingerprint",
            Self::None => "none",
        }
    }
}

/// A binding strength in `[0, 1]`.
///
/// `1.0` is reachable only through [`MatchScore::hard_exact`]. Every soft basis is clamped to
/// `0.99`, which makes `match == 1.0` an exact iff-test for a hard binding rather than a convention
/// a caller has to remember.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct MatchScore(f64);

pub const SOFT_MATCH_CEILING: f64 = 0.99;

impl MatchScore {
    pub const fn hard_exact() -> Self {
        Self(1.0)
    }

    pub const fn zero() -> Self {
        Self(0.0)
    }

    /// Clamps into `[0, 0.99]`. A non-finite input becomes zero rather than propagating.
    pub fn soft(value: f64) -> Self {
        if !value.is_finite() || value <= 0.0 {
            return Self(0.0);
        }
        Self(value.min(SOFT_MATCH_CEILING))
    }

    pub const fn value(self) -> f64 {
        self.0
    }
}

/// The confidence class of a recovered GenoMark payload, as GenoTrace consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkConfidence {
    /// Two or more non-overlapping blocks agreed. Proof level `directly_observed`.
    Strong,
    /// One block CRC-passed. Proof level `inferred`, and `inferred` never verifies.
    Single,
}

/// The evidence Stage 3 produced, with the numbers attached.
///
/// [`Self::class`] is the projection the status mapping consumes. Adding a variant here without
/// adding one there is a compile error, which is the point.
#[derive(Debug, Clone)]
pub enum BindingEvaluation {
    /// The signed content digest recomputes over these file bytes.
    HardExactContent,
    /// The container bytes moved; the signed decoded-audio digest still recomputes.
    HardExactDecodedAudioOnly,
    HardMismatch {
        expected: String,
    },
    /// Route 3(c): the signed digest does not recompute, and both the GenoMark payload and the
    /// signed reference constellation say this is the same work over a lossy path.
    ///
    /// `expected` is retained so the failed digest is still reported. Never confuse this with a
    /// hard binding: [`Self::score`] is a coverage figure capped at 0.99 and
    /// [`Self::proof_level`] is `inferred`.
    HardMismatchSoftAffirmed {
        expected: String,
        score: MatchScore,
        coverage: f64,
        blocks_agreeing: usize,
        blocks_present: usize,
        false_positive_rate: FalsePositiveRate,
    },
    /// The same evidence with no null-test row behind it. Reported as
    /// `soft_binding_false_positive_rate_unknown` rather than as `changed`: the soft binding did
    /// affirm, so calling the file tampered would be the same false accusation in a new place.
    HardMismatchSoftAffirmedUnpriced {
        expected: String,
        score: MatchScore,
        coverage: f64,
        blocks_agreeing: usize,
        blocks_present: usize,
    },
    /// A GenoMark soft binding. Cannot be built without a measured false-positive rate.
    SoftMark {
        confidence: MarkConfidence,
        score: MatchScore,
        blocks_agreeing: usize,
        blocks_present: usize,
        false_positive_rate: FalsePositiveRate,
    },
    /// A GenoMark soft binding with no null-test row behind it.
    SoftMarkUnpriced {
        confidence: MarkConfidence,
        score: MatchScore,
        blocks_agreeing: usize,
        blocks_present: usize,
    },
    /// Perceptual similarity. Always `inferred`, never verifies.
    SoftFingerprint {
        score: MatchScore,
        coherence_ratio: f64,
        false_positive_rate: Option<FalsePositiveRate>,
    },
    NoEvidence,
}

impl BindingEvaluation {
    pub fn class(&self, threshold: f64) -> BindingClass {
        match self {
            Self::HardExactContent => BindingClass::HardExactContent,
            Self::HardExactDecodedAudioOnly => BindingClass::HardExactDecodedAudioOnly,
            Self::HardMismatch { .. } => BindingClass::HardMismatch,
            Self::HardMismatchSoftAffirmed { .. } => BindingClass::HardMismatchSoftAffirmed,
            Self::HardMismatchSoftAffirmedUnpriced { .. } => {
                BindingClass::SoftMarkFalsePositiveRateUnknown
            }
            Self::SoftMark {
                confidence: MarkConfidence::Single,
                ..
            } => BindingClass::SoftMarkInferredOnly,
            Self::SoftMark {
                confidence: MarkConfidence::Strong,
                score,
                ..
            } => {
                if score.value() >= threshold {
                    BindingClass::SoftMarkStrongAtOrAboveThreshold
                } else {
                    BindingClass::SoftMarkStrongBelowThreshold
                }
            }
            // An unpriced mark that could not clear the threshold anyway reports the thinner
            // evidence, because re-recording longer audio is the remedy there, not running a bench.
            Self::SoftMarkUnpriced {
                confidence, score, ..
            } => match confidence {
                MarkConfidence::Single => BindingClass::SoftMarkInferredOnly,
                MarkConfidence::Strong => {
                    if score.value() >= threshold {
                        BindingClass::SoftMarkFalsePositiveRateUnknown
                    } else {
                        BindingClass::SoftMarkStrongBelowThreshold
                    }
                }
            },
            Self::SoftFingerprint { .. } => BindingClass::SoftFingerprintInferred,
            Self::NoEvidence => BindingClass::NoBindingEvidence,
        }
    }

    pub const fn basis(&self) -> MatchBasis {
        match self {
            Self::HardExactContent | Self::HardExactDecodedAudioOnly => MatchBasis::HardExact,
            Self::SoftMark { .. } | Self::SoftMarkUnpriced { .. } => MatchBasis::Genomark,
            Self::SoftFingerprint { .. } => MatchBasis::Fingerprint,
            Self::HardMismatchSoftAffirmed { .. }
            | Self::HardMismatchSoftAffirmedUnpriced { .. } => MatchBasis::MarkAndFingerprint,
            // A mismatched hard binding produced no match at all; the basis is the absence of one.
            Self::HardMismatch { .. } | Self::NoEvidence => MatchBasis::None,
        }
    }

    pub const fn score(&self) -> MatchScore {
        match self {
            Self::HardExactContent | Self::HardExactDecodedAudioOnly => MatchScore::hard_exact(),
            Self::SoftMark { score, .. }
            | Self::SoftMarkUnpriced { score, .. }
            | Self::SoftFingerprint { score, .. }
            | Self::HardMismatchSoftAffirmed { score, .. }
            | Self::HardMismatchSoftAffirmedUnpriced { score, .. } => *score,
            Self::HardMismatch { .. } | Self::NoEvidence => MatchScore::zero(),
        }
    }

    /// SDK_SPEC's `BindingReport.proofLevel`: a hard binding is observed, a `strong` mark is
    /// observed, a `single` mark and every fingerprint are inferred and never higher.
    pub const fn proof_level(&self) -> ProofLevel {
        match self {
            Self::HardExactContent | Self::HardExactDecodedAudioOnly => {
                ProofLevel::DirectlyObserved
            }
            Self::SoftMark { confidence, .. } | Self::SoftMarkUnpriced { confidence, .. } => {
                match confidence {
                    MarkConfidence::Strong => ProofLevel::DirectlyObserved,
                    MarkConfidence::Single => ProofLevel::Inferred,
                }
            }
            Self::SoftFingerprint { .. } => ProofLevel::Inferred,
            // IMPORTANT: `inferred`, never higher. The bytes demonstrably moved; what was observed
            // is that the recording is the same one, which is an inference from two soft signals.
            Self::HardMismatchSoftAffirmed { .. }
            | Self::HardMismatchSoftAffirmedUnpriced { .. } => ProofLevel::Inferred,
            // Nothing was observed to bind, so nothing was observed.
            Self::HardMismatch { .. } | Self::NoEvidence => ProofLevel::UnknownUnobserved,
        }
    }

    pub const fn false_positive_rate(&self) -> Option<FalsePositiveRate> {
        match self {
            Self::SoftMark {
                false_positive_rate,
                ..
            } => Some(*false_positive_rate),
            Self::SoftFingerprint {
                false_positive_rate,
                ..
            } => *false_positive_rate,
            Self::HardMismatchSoftAffirmed {
                false_positive_rate,
                ..
            } => Some(*false_positive_rate),
            Self::HardExactContent
            | Self::HardExactDecodedAudioOnly
            | Self::HardMismatch { .. }
            | Self::HardMismatchSoftAffirmedUnpriced { .. }
            | Self::SoftMarkUnpriced { .. }
            | Self::NoEvidence => None,
        }
    }

    fn expected_digest(&self) -> Option<&str> {
        match self {
            Self::HardMismatch { expected }
            | Self::HardMismatchSoftAffirmed { expected, .. }
            | Self::HardMismatchSoftAffirmedUnpriced { expected, .. } => Some(expected.as_str()),
            Self::HardExactContent
            | Self::HardExactDecodedAudioOnly
            | Self::SoftMark { .. }
            | Self::SoftMarkUnpriced { .. }
            | Self::SoftFingerprint { .. }
            | Self::NoEvidence => None,
        }
    }

    /// The threshold the reported [`Self::score`] was actually gated against.
    ///
    /// IMPORTANT: a GenoMark coverage score and a reference-constellation coverage score are
    /// different quantities with different gates. Printing one beside the other's threshold, which
    /// the caller's single `soft_binding_threshold` would do, is a confidently wrong sentence.
    pub const fn threshold(&self, soft_binding_threshold: f64) -> f64 {
        match self {
            Self::HardMismatchSoftAffirmed { .. }
            | Self::HardMismatchSoftAffirmedUnpriced { .. } => {
                crate::fingerprint::reference::AFFIRMATION_COVERAGE
            }
            Self::HardExactContent
            | Self::HardExactDecodedAudioOnly
            | Self::HardMismatch { .. }
            | Self::SoftMark { .. }
            | Self::SoftMarkUnpriced { .. }
            | Self::SoftFingerprint { .. }
            | Self::NoEvidence => soft_binding_threshold,
        }
    }

    fn detail(&self) -> String {
        match self {
            Self::HardExactContent => "signed content digest recomputes exactly".to_string(),
            Self::HardExactDecodedAudioOnly => {
                "container bytes differ; signed decoded-audio digest recomputes exactly".to_string()
            }
            Self::HardMismatch { .. } => "signed hard binding does not recompute".to_string(),
            Self::SoftMark {
                blocks_agreeing,
                blocks_present,
                ..
            }
            | Self::SoftMarkUnpriced {
                blocks_agreeing,
                blocks_present,
                ..
            } => format!("GenoMark, {blocks_agreeing}/{blocks_present} blocks"),
            Self::SoftFingerprint {
                coherence_ratio, ..
            } => format!("fingerprint, coherence ratio {coherence_ratio:.4}"),
            // Names both signals, because the point of the row is that a consumer can tell an
            // exact-bytes verification from an inferred one without reading the spec.
            Self::HardMismatchSoftAffirmed {
                coverage,
                blocks_agreeing,
                blocks_present,
                ..
            }
            | Self::HardMismatchSoftAffirmedUnpriced {
                coverage,
                blocks_agreeing,
                blocks_present,
                ..
            } => format!(
                "signed hard binding does not recompute; GenoMark {blocks_agreeing}/{blocks_present} blocks and the signed reference constellation agree, {:.0}% of the presented audio aligned",
                coverage * 100.0
            ),
            Self::NoEvidence => "no binding evidence recovered".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    HardHash,
    SoftGenomark,
    SoftFingerprint,
    SoftMarkAndFingerprint,
    None,
}

#[derive(Debug, Clone, Serialize)]
pub struct BindingReport {
    pub kind: BindingKind,
    pub r#match: f64,
    pub proof_level: ProofLevel,
    pub threshold: f64,
    /// Required non-null for every soft basis that could have verified. A confidence with no
    /// false-positive rate behind it is not actionable.
    pub false_positive_rate_at_match: Option<f64>,
    pub false_positive_rate_basis: Option<&'static str>,
    pub expected_sha256: Option<String>,
    pub observed_sha256: String,
    pub detail: String,
}

/// A `audio_provenance_manifest::Finding` in a shape that serialises.
#[derive(Debug, Clone, Serialize)]
pub struct FindingReport {
    pub severity: &'static str,
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl From<&Finding> for FindingReport {
    fn from(finding: &Finding) -> Self {
        Self {
            severity: finding.severity().as_str(),
            code: finding.code(),
            path: finding.path().to_string(),
            message: finding.message().to_string(),
        }
    }
}

pub fn findings_report(findings: &[Finding]) -> Vec<FindingReport> {
    findings.iter().map(FindingReport::from).collect()
}

/// The five rungs, named as CLI_SPEC prints them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryMethod {
    EmbeddedManifest,
    SidecarManifest,
    ContentHashLookup,
    DecodedAudioHashLookup,
    GenomarkRecovery,
    FingerprintSearch,
}

impl RecoveryMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmbeddedManifest => "embedded_manifest",
            Self::SidecarManifest => "sidecar_manifest",
            Self::ContentHashLookup => "content_hash_lookup",
            Self::DecodedAudioHashLookup => "decoded_audio_hash_lookup",
            Self::GenomarkRecovery => "genomark_recovery",
            Self::FingerprintSearch => "fingerprint_search",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepOutcome {
    Hit,
    Miss,
    /// The rung was deliberately not run: `--offline`, no index configured, an opt-in withheld.
    /// Never sets `incomplete`.
    Skipped,
    Degraded,
    /// The rung was meant to run and could not. This is what sets `incomplete`.
    Unavailable,
}

impl StepOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Skipped => "skipped",
            Self::Degraded => "degraded",
            Self::Unavailable => "unavailable",
        }
    }

    /// A rung that could not run leaves the search incomplete. A rung that was not asked to run
    /// does not.
    pub const fn leaves_incomplete(self) -> bool {
        matches!(self, Self::Unavailable)
    }

    /// Whether the rung actually reached its conclusion.
    pub const fn ran_to_completion(self) -> bool {
        matches!(self, Self::Hit | Self::Miss | Self::Degraded)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryStep {
    pub method: RecoveryMethod,
    pub outcome: StepOutcome,
    pub proof_level: ProofLevel,
    pub duration_ms: f64,
    pub detail: String,
}

/// A candidate the ladder found and then put down, with the reason.
#[derive(Debug, Clone, Serialize)]
pub struct DiscardedCandidate {
    pub method: RecoveryMethod,
    pub record_digest: Option<String>,
    pub reason: &'static str,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryReport {
    /// True when every rung reached a conclusion. `not_found` with `exhausted: false` says the
    /// search stopped early, which is why `not_found` never means "no manifest exists".
    pub exhausted: bool,
    pub discarded: Vec<DiscardedCandidate>,
    pub diagnostics: Vec<FindingReport>,
}

/// Locator recovery is deliberately separate from recording association. A
/// watermark may name a signed record while the recording presented fails that
/// record's signed binding; in that case `located` is true and association is
/// rejected.
#[derive(Debug, Clone, Serialize)]
pub struct MarkRecoveryReport {
    /// A CRC-valid mark was recovered from the presented audio. This says only that a locator was
    /// observed; it does not say the recording matched the record that locator names.
    pub recovered: bool,
    /// The mark rung used that locator to recover the admitted candidate record.
    pub located: bool,
    pub method: Option<RecoveryMethod>,
    /// Compatibility spelling of `located`: true only when record recovery ran through the mark
    /// rung, not merely because a sidecar was available while a mark also decoded.
    pub recovered_via_mark: bool,
    /// Whether the recovered locator named the candidate record whose binding was evaluated.
    pub record_locator_matched: bool,
    pub proof_level: ProofLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingAssociationStatus {
    Exact,
    Associated,
    Rejected,
    InsufficientEvidence,
    NotEvaluated,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordingAssociationReport {
    pub status: RecordingAssociationStatus,
    pub established: bool,
    pub proof_level: ProofLevel,
    pub r#match: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignatureReport {
    pub algorithm: String,
    pub canonicalization: String,
    pub signer_id: Option<String>,
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RegistrySourceReport {
    pub name: String,
    pub kind: &'static str,
    pub location: String,
}

impl From<&RegistrySource> for RegistrySourceReport {
    fn from(source: &RegistrySource) -> Self {
        Self {
            name: source.name().to_string(),
            kind: source.kind().as_str(),
            location: source.location().to_string(),
        }
    }
}

/// The verdict and everything behind it.
///
/// `identity` and `identity_proof_level` both come from one [`Verdict`], so SDK_SPEC's
/// "non-null IFF `externally_verified`" cannot be broken by an assembly path.
#[derive(Debug, Clone, Serialize)]
pub struct VerifyResult {
    pub status: VerificationStatus,
    pub reason: &'static str,
    pub identity: Option<String>,
    pub identity_proof_level: ProofLevel,
    pub identity_authority: Option<String>,
    pub signed_at: Option<String>,
    pub r#match: f64,
    pub match_basis: MatchBasis,
    pub binding: BindingReport,
    pub signature: Option<SignatureReport>,
    pub trace: Vec<RecoveryStep>,
    pub recovery: RecoveryReport,
    pub mark_recovery: MarkRecoveryReport,
    pub recording_association: RecordingAssociationReport,
    pub findings: Vec<FindingReport>,
    #[serde(skip)]
    pub manifest: Option<Manifest>,
    pub registry: Option<RegistrySourceReport>,
    pub content_sha256: String,
    pub content_bytes: u64,
    pub decoded_audio_sha256: Option<String>,
    pub method: Option<RecoveryMethod>,
    pub incomplete: bool,
}

/// Assembles a result from a verdict plus the evidence, and is the only place that writes
/// `identity`, `match` and `match_basis`.
pub(crate) struct ResultAssembly {
    pub verdict: Verdict,
    pub binding: Option<BindingEvaluation>,
    pub threshold: f64,
    pub observed_sha256: String,
    pub content_bytes: u64,
    pub decoded_audio_sha256: Option<String>,
    pub manifest: Option<Manifest>,
    pub signature: Option<SignatureReport>,
    pub anchored_identity: Option<(String, String)>,
    pub trace: Vec<RecoveryStep>,
    pub recovery: RecoveryReport,
    pub findings: Vec<FindingReport>,
    pub registry: Option<RegistrySourceReport>,
    pub method: Option<RecoveryMethod>,
    pub incomplete: bool,
}

impl ResultAssembly {
    pub(crate) fn finish(self) -> VerifyResult {
        let discloses = self.verdict.discloses_identity();
        let (identity, authority) = match (discloses, self.anchored_identity) {
            (true, Some((identity, authority))) => (Some(identity), Some(authority)),
            // A verdict that discloses with no resolved name is not a name; it is nothing.
            (true, None) | (false, _) => (None, None),
        };
        let identity_proof_level = if identity.is_some() {
            ProofLevel::ExternallyVerified
        } else {
            ProofLevel::UnknownUnobserved
        };

        // `match` and `match_basis` are properties of the binding that was actually evaluated. A
        // status that did not reach binding evaluation has neither.
        let (basis, score, proof_level, fpr, expected, detail, threshold) = match &self.binding {
            Some(evaluation) => (
                evaluation.basis(),
                evaluation.score(),
                evaluation.proof_level(),
                evaluation.false_positive_rate(),
                evaluation.expected_digest().map(str::to_string),
                evaluation.detail(),
                evaluation.threshold(self.threshold),
            ),
            None => (
                MatchBasis::None,
                MatchScore::zero(),
                ProofLevel::UnknownUnobserved,
                None,
                None,
                self.verdict.reason().as_str().to_string(),
                self.threshold,
            ),
        };

        let kind = match basis {
            MatchBasis::HardExact => BindingKind::HardHash,
            MatchBasis::Genomark => BindingKind::SoftGenomark,
            MatchBasis::Fingerprint => BindingKind::SoftFingerprint,
            MatchBasis::MarkAndFingerprint => BindingKind::SoftMarkAndFingerprint,
            MatchBasis::None => BindingKind::None,
        };

        let recording_association = match self
            .binding
            .as_ref()
            .map(|evaluation| evaluation.class(self.threshold))
        {
            Some(BindingClass::HardExactContent | BindingClass::HardExactDecodedAudioOnly) => {
                RecordingAssociationReport {
                    status: RecordingAssociationStatus::Exact,
                    established: true,
                    proof_level,
                    r#match: score.value(),
                }
            }
            Some(
                BindingClass::HardMismatchSoftAffirmed
                | BindingClass::SoftMarkStrongAtOrAboveThreshold,
            ) => RecordingAssociationReport {
                status: RecordingAssociationStatus::Associated,
                established: true,
                proof_level,
                r#match: score.value(),
            },
            Some(
                BindingClass::HardMismatch
                | BindingClass::NoBindingEvidence
                | BindingClass::SoftMarkStrongBelowThreshold,
            ) => RecordingAssociationReport {
                status: RecordingAssociationStatus::Rejected,
                established: false,
                proof_level,
                r#match: score.value(),
            },
            Some(_) => RecordingAssociationReport {
                status: RecordingAssociationStatus::InsufficientEvidence,
                established: false,
                proof_level,
                r#match: score.value(),
            },
            None => RecordingAssociationReport {
                status: RecordingAssociationStatus::NotEvaluated,
                established: false,
                proof_level: ProofLevel::UnknownUnobserved,
                r#match: 0.0,
            },
        };

        let recovered_via_mark =
            self.method == Some(RecoveryMethod::GenomarkRecovery) && self.manifest.is_some();
        let mark_step = self
            .trace
            .iter()
            .find(|step| step.method == RecoveryMethod::GenomarkRecovery);
        let binding_used_mark =
            matches!(basis, MatchBasis::Genomark | MatchBasis::MarkAndFingerprint);
        let recovered_mark_named_another_record = self
            .findings
            .iter()
            .any(|finding| finding.code == "recovered_mark_names_another_record");
        let recovered = binding_used_mark
            || recovered_mark_named_another_record
            || mark_step.is_some_and(|step| step.outcome == StepOutcome::Hit);
        let record_locator_matched = binding_used_mark || recovered_via_mark;
        let mark_recovery = MarkRecoveryReport {
            recovered,
            located: recovered_via_mark,
            method: self.method,
            recovered_via_mark,
            record_locator_matched,
            proof_level: if recovered {
                mark_step.map_or(proof_level, |step| step.proof_level)
            } else {
                ProofLevel::UnknownUnobserved
            },
        };

        let signed_at = match self.verdict.status() {
            VerificationStatus::NotFound => None,
            _ => self
                .manifest
                .as_ref()
                .and_then(|manifest| manifest.signed_at().map(str::to_string)),
        };

        VerifyResult {
            status: self.verdict.status(),
            reason: self.verdict.reason().as_str(),
            identity,
            identity_proof_level,
            identity_authority: authority,
            signed_at,
            r#match: score.value(),
            match_basis: basis,
            binding: BindingReport {
                kind,
                r#match: score.value(),
                proof_level,
                threshold,
                false_positive_rate_at_match: fpr.map(FalsePositiveRate::value),
                false_positive_rate_basis: fpr
                    .map(|rate| rate.rate_basis())
                    .map(crate::nulltest::RateBasis::as_str),
                expected_sha256: expected,
                observed_sha256: self.observed_sha256.clone(),
                detail,
            },
            signature: self.signature,
            trace: self.trace,
            recovery: self.recovery,
            mark_recovery,
            recording_association,
            findings: self.findings,
            manifest: self.manifest,
            registry: self.registry,
            content_sha256: self.observed_sha256,
            content_bytes: self.content_bytes,
            decoded_audio_sha256: self.decoded_audio_sha256,
            method: self.method,
            incomplete: self.incomplete || self.verdict.incomplete(),
        }
    }
}
