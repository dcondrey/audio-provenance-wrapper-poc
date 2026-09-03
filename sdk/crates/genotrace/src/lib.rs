//! GenoTrace: a bare audio file in, an admitted signed manifest and one of four provenance
//! statuses out.
//!
//! # `verify` never returns an error for a provenance outcome
//!
//! [`verify`] and [`verify_bytes`] return [`GenoTraceError`] only for caller errors: an unreadable
//! path, an oversize input, a malformed option. Every provenance condition, INCLUDING A REGISTRY
//! OUTAGE, resolves to one of `verified` / `changed` / `untrusted` / `not_found` plus
//! [`VerifyResult::incomplete`]. That rule is what keeps a network fault from becoming a verdict,
//! and `audio_provenance_registry::Lookup` keeps the distinction at the type level all the way up.
//!
//! # `not_found` never means "no manifest exists"
//!
//! It means none was recovered. [`RecoveryReport::exhausted`] and [`VerifyResult::incomplete`] say
//! how hard the search looked.
//!
//! # Acoustic re-recording is unsupported
//!
//! Audio played through a speaker and captured by a microphone returns `not_found`. The failure is
//! structural, not a tuning deficit; `genomark::ACOUSTIC_RERECORDING` is the literal string
//! `"unsupported"`, never null.
//!
//! # A fingerprint hit can never verify
//!
//! Rung 5 recovers no binding from the audio at all; it guesses a manifest from perceptual
//! similarity. It is always proof level `inferred`, it emits no candidate under the default policy,
//! and even under [`InferredAssociationPolicy::EmitCandidate`] the status mapping has no arm that
//! takes it to `verified`.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod admission;
pub mod container;
pub mod error;
pub mod fingerprint;
pub mod ingest;
pub mod ladder;
pub mod nulltest;
pub mod result;
pub mod status;
pub mod trust;

use std::path::{Path, PathBuf};

use audio_provenance_registry::RegistryBackend;
use genomark::GenoMark;

pub use error::GenoTraceError;
pub use fingerprint::index::{
    FileFingerprintIndex, FingerprintIndex, MemoryFingerprintIndex, Posting,
};
pub use fingerprint::reference::{
    AFFIRMATION_COVERAGE, Affirmation, ReferenceFingerprint, reference_fingerprint,
};
pub use ingest::{IngestLimits, Ingested, decoded_audio_sha256, ingest_bytes, ingest_path};
pub use ladder::{DecodedAudioIndex, InferredAssociationPolicy, SidecarPolicy};
pub use nulltest::{FalsePositiveRate, NullTestTable, RateBasis};
pub use result::{
    BindingKind, BindingReport, DEFAULT_SOFT_BINDING_THRESHOLD, DiscardedCandidate,
    MarkRecoveryReport, MatchBasis, RecordingAssociationReport, RecordingAssociationStatus,
    RecoveryMethod, RecoveryReport, RecoveryStep, SignatureReport, StepOutcome, VerifyResult,
};
pub use status::{
    BindingClass, Exhaustion, RejectionReason, SignatureOutcome, StatusInputs, StatusReason,
    TrustOutcome, Verdict, derive_status,
};
pub use trust::{FileTrustStore, NoTrustAnchors, TrustAnchor, TrustResolution, TrustStore};

use crate::fingerprint::score::ScoreLimits;
use crate::ladder::{LadderInputs, Rejected};
use crate::result::{RecoveryReport as Report, ResultAssembly, SignatureReport as SigReport};
use crate::status::{Exhaustion as Exh, StatusInputs as Inputs};

/// Everything a verification may be pointed at.
///
/// Borrows rather than owns, so a caller composing several verifications against one registry and
/// one trust store pays for neither twice.
#[derive(Debug)]
pub struct VerifyOptions<'a> {
    registry: Option<&'a dyn RegistryBackend>,
    decoded_audio_index: Option<&'a dyn DecodedAudioIndex>,
    fingerprint_index: Option<&'a dyn FingerprintIndex>,
    trust_store: Option<&'a dyn TrustStore>,
    null_test: Option<&'a NullTestTable>,
    genomark: Option<&'a GenoMark>,
    sidecar: SidecarPolicy,
    offline: bool,
    soft_binding_threshold: f64,
    inferred: InferredAssociationPolicy,
    limits: IngestLimits,
    score_limits: ScoreLimits,
}

impl Default for VerifyOptions<'_> {
    fn default() -> Self {
        Self {
            registry: None,
            decoded_audio_index: None,
            fingerprint_index: None,
            trust_store: None,
            null_test: None,
            genomark: None,
            sidecar: SidecarPolicy::Conventional,
            offline: false,
            soft_binding_threshold: DEFAULT_SOFT_BINDING_THRESHOLD,
            inferred: InferredAssociationPolicy::DiagnosticsOnly,
            limits: IngestLimits::default(),
            score_limits: ScoreLimits::default(),
        }
    }
}

impl<'a> VerifyOptions<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_registry(mut self, registry: &'a dyn RegistryBackend) -> Self {
        self.registry = Some(registry);
        self
    }

    #[must_use]
    pub fn with_decoded_audio_index(mut self, index: &'a dyn DecodedAudioIndex) -> Self {
        self.decoded_audio_index = Some(index);
        self
    }

    #[must_use]
    pub fn with_fingerprint_index(mut self, index: &'a dyn FingerprintIndex) -> Self {
        self.fingerprint_index = Some(index);
        self
    }

    #[must_use]
    pub fn with_trust_store(mut self, store: &'a dyn TrustStore) -> Self {
        self.trust_store = Some(store);
        self
    }

    /// Supplies the measured null-test table. Without one, no soft binding can reach `verified`.
    #[must_use]
    pub fn with_null_test(mut self, table: &'a NullTestTable) -> Self {
        self.null_test = Some(table);
        self
    }

    #[must_use]
    pub fn with_genomark(mut self, genomark: &'a GenoMark) -> Self {
        self.genomark = Some(genomark);
        self
    }

    #[must_use]
    pub fn with_sidecar(mut self, policy: SidecarPolicy) -> Self {
        self.sidecar = policy;
        self
    }

    /// Registry rungs become `skipped`, never `unavailable`, so `--offline` alone never sets
    /// `incomplete`.
    #[must_use]
    pub const fn offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    #[must_use]
    pub const fn with_inferred_association(mut self, policy: InferredAssociationPolicy) -> Self {
        self.inferred = policy;
        self
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: IngestLimits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub const fn with_score_limits(mut self, limits: ScoreLimits) -> Self {
        self.score_limits = limits;
        self
    }

    pub fn with_soft_binding_threshold(mut self, threshold: f64) -> Result<Self, GenoTraceError> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(GenoTraceError::InvalidOption {
                option: "soft_binding_threshold",
                reason: "must be a number in [0, 1]".to_string(),
            });
        }
        self.soft_binding_threshold = threshold;
        Ok(self)
    }
}

/// Verifies a file on disk.
pub fn verify(path: &Path, options: &VerifyOptions<'_>) -> Result<VerifyResult, GenoTraceError> {
    let ingested = ingest_path(path, options.limits)?;
    Ok(run(&ingested, Some(path), options))
}

/// Verifies bytes already in hand. Sidecar rungs are then unavailable to the search, because there
/// is no real directory to resolve them against and no path from inside a manifest is ever
/// followed.
pub fn verify_bytes(
    bytes: Vec<u8>,
    options: &VerifyOptions<'_>,
) -> Result<VerifyResult, GenoTraceError> {
    let ingested = ingest_bytes(bytes, options.limits)?;
    Ok(run(&ingested, None, options))
}

/// What GenoTrace found, with no verdict and no trust evaluation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InspectReport {
    pub container: &'static str,
    pub content_sha256: String,
    pub content_bytes: u64,
    pub decoded_audio_sha256: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub duration_seconds: f64,
    pub trace: Vec<RecoveryStep>,
    pub recovery: Report,
    pub manifest_schema: Option<&'static str>,
    pub signer_id: Option<String>,
    pub incomplete: bool,
}

pub fn inspect(path: &Path, options: &VerifyOptions<'_>) -> Result<InspectReport, GenoTraceError> {
    let ingested = ingest_path(path, options.limits)?;
    let genomark = GenoMark::public();
    let outcome = ladder::run(&LadderInputs {
        ingested: &ingested,
        source_path: Some(path),
        registry: options.registry,
        decoded_audio_index: options.decoded_audio_index,
        fingerprint_index: options.fingerprint_index,
        genomark: options.genomark.unwrap_or(&genomark),
        offline: options.offline,
        sidecar: options.sidecar.clone(),
        inferred: options.inferred,
        score_limits: options.score_limits,
    })?;

    let audio = ingested.audio();
    Ok(InspectReport {
        container: ingested.container().as_str(),
        content_sha256: ingested.content_sha256().to_string(),
        content_bytes: ingested.content_bytes(),
        decoded_audio_sha256: ingested.decoded_audio_sha256(),
        sample_rate: audio.sample_rate(),
        channels: audio.channels(),
        duration_seconds: audio.duration_seconds(),
        trace: outcome.trace,
        recovery: Report {
            exhausted: outcome.exhausted,
            discarded: outcome.discarded,
            diagnostics: result::findings_report(&outcome.diagnostics),
        },
        manifest_schema: outcome
            .candidate
            .as_ref()
            .map(|candidate| candidate.manifest.schema().id()),
        signer_id: outcome
            .candidate
            .as_ref()
            .and_then(|candidate| candidate.manifest.signer().signer_id().map(str::to_string)),
        incomplete: outcome.incomplete,
    })
}

fn run(ingested: &Ingested, path: Option<&Path>, options: &VerifyOptions<'_>) -> VerifyResult {
    let default_mark = GenoMark::public();
    let genomark = options.genomark.unwrap_or(&default_mark);
    let empty_table = NullTestTable::empty();
    let null_test = options.null_test.unwrap_or(&empty_table);
    let no_anchors = NoTrustAnchors;
    let trust_store: &dyn TrustStore = options.trust_store.unwrap_or(&no_anchors);

    let sidecar = match (path, &options.sidecar) {
        (None, _) => SidecarPolicy::Disabled,
        (Some(_), policy) => policy.clone(),
    };

    let ladder_result = ladder::run(&LadderInputs {
        ingested,
        source_path: path,
        registry: options.registry,
        decoded_audio_index: options.decoded_audio_index,
        fingerprint_index: options.fingerprint_index,
        genomark,
        offline: options.offline,
        sidecar,
        inferred: options.inferred,
        score_limits: options.score_limits,
    });

    let outcome = match ladder_result {
        Ok(outcome) => outcome,
        // A fingerprint index that will not parse is a configuration fault, not a provenance
        // finding. It marks the search incomplete rather than becoming a verdict.
        Err(error) => {
            let mut outcome = ladder::LadderOutcome {
                exhausted: false,
                incomplete: true,
                ..ladder::LadderOutcome::default()
            };
            outcome
                .diagnostics
                .push(audio_provenance_manifest::Finding::warning(
                    "recovery_rung_failed",
                    "$",
                    error.to_string(),
                ));
            outcome
        }
    };

    let report = Report {
        exhausted: outcome.exhausted,
        discarded: outcome.discarded,
        diagnostics: result::findings_report(&outcome.diagnostics),
    };
    let registry_report = options
        .registry
        .map(|registry| result::RegistrySourceReport::from(registry.source()));
    let observed = ingested.content_sha256().to_string();
    let decoded = ingested.decoded_audio_sha256();

    let common = |inputs: Inputs,
                  assembly: PartialAssembly,
                  extra: Vec<audio_provenance_manifest::Finding>|
     -> VerifyResult {
        let verdict = derive_status(inputs);
        let mut findings = outcome.diagnostics.clone();
        findings.extend(extra);
        ResultAssembly {
            verdict,
            binding: assembly.binding,
            threshold: options.soft_binding_threshold,
            observed_sha256: observed.clone(),
            content_bytes: ingested.content_bytes(),
            decoded_audio_sha256: Some(decoded.clone()),
            manifest: assembly.manifest,
            signature: assembly.signature,
            anchored_identity: assembly.anchored_identity,
            trace: outcome.trace.clone(),
            recovery: report.clone(),
            findings: result::findings_report(&findings),
            registry: registry_report.clone(),
            method: assembly.method,
            incomplete: outcome.incomplete,
        }
        .finish()
    };

    if let Some(rejected) = outcome.rejected {
        let inputs = match &rejected {
            Rejected::Integrity { rejection, .. } => Inputs::CandidateRejected(*rejection),
            Rejected::Signature { signature, .. } => Inputs::Candidate {
                signature: *signature,
                // Nothing bound is read on a candidate whose signature failed, so there is no
                // binding to report. The class is inert: every signature arm of the mapping ignores
                // it.
                binding: BindingClass::NoBindingEvidence,
                trust: TrustOutcome::Unanchored,
            },
        };
        let code = match &rejected {
            Rejected::Integrity { rejection, .. } => match rejection {
                RejectionReason::NoncanonicalManifest => "noncanonical_manifest",
                RejectionReason::LocatorMismatch => "locator_mismatch",
                RejectionReason::AmbiguousBinding => "ambiguous_binding",
                RejectionReason::InvariantsFailed => "manifest_invariants_failed",
            },
            Rejected::Signature { signature, .. } => match signature {
                SignatureOutcome::Valid | SignatureOutcome::Invalid => "signature_invalid",
                SignatureOutcome::Absent => "signature_absent",
                SignatureOutcome::UnsupportedAlgorithm => "signature_algorithm_unsupported",
            },
        };
        let finding = audio_provenance_manifest::Finding::error(code, "$", rejected.detail());
        return common(
            inputs,
            PartialAssembly {
                method: Some(rejected.method()),
                ..PartialAssembly::default()
            },
            vec![finding],
        );
    }

    let Some(candidate) = outcome.candidate else {
        let exhaustion = if outcome.incomplete {
            Exh::RungCouldNotRun
        } else {
            Exh::AllRungsRan
        };
        return common(
            Inputs::NoCandidate(exhaustion),
            PartialAssembly::default(),
            Vec::new(),
        );
    };

    let manifest = candidate.manifest;
    let mut extra = Vec::new();
    let binding = ladder::evaluate_binding(
        &ladder::BindingInputs {
            manifest: &manifest,
            ingested,
            genomark,
            detection: outcome.detection.as_ref(),
            fingerprint: outcome.fingerprint.as_ref(),
            null_test,
            score_limits: options.score_limits,
            soft_binding_threshold: options.soft_binding_threshold,
        },
        &mut extra,
    );
    let trust = match manifest.key_possession() {
        Ok(proof) => trust_store.resolve(&proof),
        // The signature already verified during admission, so a failure here is a re-derivation
        // fault rather than a bad signature. It resolves nothing and anchors nothing.
        Err(_) => TrustResolution::Failed {
            reason: "signer could not be re-derived from the admitted manifest".to_string(),
        },
    };
    let signature = SigReport {
        algorithm: format!("{:?}", manifest.signature().algorithm),
        canonicalization: audio_provenance_core::CANONICALIZATION_ID.to_string(),
        signer_id: manifest.signer().signer_id().map(str::to_string),
        valid: true,
    };
    let anchored_identity = trust
        .anchor()
        .map(|anchor| (anchor.identity.clone(), anchor.authority.clone()));

    // A container-only edit is reported, not swallowed. The samples are bit-identical to what was
    // signed, so the status is `verified`; the fact that the file bytes moved is a finding.
    if matches!(
        binding,
        result::BindingEvaluation::HardExactDecodedAudioOnly
    ) {
        extra.push(audio_provenance_manifest::Finding::warning(
            "container_bytes_changed",
            "$.hard_binding",
            "the signed content digest no longer recomputes, but the signed decoded-audio digest \
             does: the container was rewritten and no sample moved",
        ));
    }
    if let TrustResolution::Failed { reason } = &trust {
        extra.push(audio_provenance_manifest::Finding::warning(
            "trust_anchor_rejected",
            "$.portable_signature",
            reason.clone(),
        ));
    }

    common(
        Inputs::Candidate {
            signature: SignatureOutcome::Valid,
            binding: binding.class(options.soft_binding_threshold),
            trust: trust.outcome(),
        },
        PartialAssembly {
            binding: Some(binding),
            manifest: Some(*manifest),
            signature: Some(signature),
            anchored_identity,
            method: Some(candidate.method),
        },
        extra,
    )
}

#[derive(Default)]
struct PartialAssembly {
    binding: Option<result::BindingEvaluation>,
    manifest: Option<audio_provenance_manifest::Manifest>,
    signature: Option<SigReport>,
    anchored_identity: Option<(String, String)>,
    method: Option<RecoveryMethod>,
}

/// The conventional sidecar paths for an asset, in the order rung 1 reads them.
pub fn sidecar_paths(path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::with_capacity(2);
    let mut adjacent = path.as_os_str().to_owned();
    adjacent.push(".audio-provenance.json");
    out.push(PathBuf::from(adjacent));
    if let (Some(parent), Some(stem)) = (path.parent(), path.file_stem()) {
        let mut name = stem.to_owned();
        name.push(".json");
        out.push(parent.join(".audio-provenance").join(name));
    }
    out
}
