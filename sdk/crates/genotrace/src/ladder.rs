//! The five-rung recovery ladder and its arbitration.
//!
//! The rung determines WHETHER a manifest was recovered and WHO asserted the association. It does
//! not by itself decide verified-versus-changed; the hard-binding recomputation does. Method and
//! binding basis are independent axes, and this module keeps them that way.

use std::path::{Path, PathBuf};
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;

use audio_provenance_audio::AudioBuffer;
use audio_provenance_core::{ProofLevel, locator_from_signed_manifest, parse_signing_input};
use audio_provenance_manifest::{
    BindingOutcome, C2paStore, Finding, MAX_MANIFEST_BYTES, Manifest, sidecar_path_for,
};
use audio_provenance_registry::{
    ContentHash, Lookup, MarkId, RecordId, RegistryBackend, RegistryRecord,
};
use genomark::{ConfidenceClass, DetectionOutcome, GenoMark};

use crate::admission::{Admission, admit};
use crate::container::{EmbeddedOutcome, find_embedded};
use crate::error::GenoTraceError;
use crate::fingerprint::index::FingerprintIndex;
use crate::fingerprint::reference::ReferenceFingerprint;
use crate::fingerprint::score::ScoreLimits;
use crate::fingerprint::{FingerprintQuery, FingerprintSearch};
use crate::ingest::Ingested;
use crate::nulltest::NullTestTable;
use crate::result::{
    BindingEvaluation, DiscardedCandidate, MarkConfidence, MatchScore, RecoveryMethod,
    RecoveryStep, StepOutcome,
};
use crate::status::{RejectionReason, SignatureOutcome};

/// A registry index over the decoded-audio digest.
///
/// Kept separate from `RegistryBackend` rather than folded into it: the two digests are different
/// key spaces, and querying the content-hash index with a decoded-audio digest would let a
/// coincidental collision return an unrelated manifest with nothing downstream to catch it. A
/// backend that offers no such index leaves rung 3 `skipped`, never `unavailable`.
pub trait DecodedAudioIndex: std::fmt::Debug + Send + Sync {
    fn lookup_by_decoded_audio_hash(&self, digest: &ContentHash) -> Lookup<RegistryRecord>;
}

/// A manifest the ladder recovered and admitted, with how it got there.
///
/// IMPORTANT: on rungs 1 and 2 the association is asserted by the FILE, so a soft-binding failure
/// there is a false claim by the file rather than a rejected guess, and the candidate stands. Only
/// rung 4, where GenoTrace itself guessed, discards on a soft-binding failure, and it does so before
/// admission. That is why nothing here records who asserted: no admitted candidate is ever
/// discarded.
#[derive(Debug)]
pub struct Candidate {
    pub method: RecoveryMethod,
    pub manifest: Box<Manifest>,
}

/// A recovery attempt that produced something other than an admitted candidate.
#[derive(Debug)]
pub enum Rejected {
    /// A candidate whose own integrity failed. Terminal.
    Integrity {
        method: RecoveryMethod,
        rejection: RejectionReason,
        detail: String,
    },
    /// A candidate that reached the signature axis and failed it. Terminal.
    Signature {
        method: RecoveryMethod,
        signature: SignatureOutcome,
        detail: String,
    },
}

impl Rejected {
    pub const fn method(&self) -> RecoveryMethod {
        match self {
            Self::Integrity { method, .. } | Self::Signature { method, .. } => *method,
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Self::Integrity { detail, .. } | Self::Signature { detail, .. } => detail,
        }
    }
}

/// Everything the ladder produced.
#[derive(Debug, Default)]
pub struct LadderOutcome {
    pub candidate: Option<Candidate>,
    pub rejected: Option<Rejected>,
    pub trace: Vec<RecoveryStep>,
    pub discarded: Vec<DiscardedCandidate>,
    pub diagnostics: Vec<Finding>,
    pub incomplete: bool,
    pub exhausted: bool,
    pub detection: Option<DetectionOutcome>,
    pub fingerprint: Option<FingerprintSearch>,
}

/// What rung 5 is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferredAssociationPolicy {
    /// Findings only. The default: a fingerprint hit is recorded in the diagnostics and the
    /// terminal status stays `not_found`.
    DiagnosticsOnly,
    /// The rung may emit a candidate. It can still never produce `verified`.
    EmitCandidate,
}

#[derive(Debug)]
pub struct LadderInputs<'a> {
    pub ingested: &'a Ingested,
    pub source_path: Option<&'a Path>,
    pub registry: Option<&'a dyn RegistryBackend>,
    pub decoded_audio_index: Option<&'a dyn DecodedAudioIndex>,
    pub fingerprint_index: Option<&'a dyn FingerprintIndex>,
    pub genomark: &'a GenoMark,
    pub offline: bool,
    pub sidecar: SidecarPolicy,
    pub inferred: InferredAssociationPolicy,
    pub score_limits: ScoreLimits,
}

/// Where a sidecar may be read from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SidecarPolicy {
    /// The conventional names beside the input file.
    #[default]
    Conventional,
    /// One explicit path, still resolved against the real filesystem and never against a path
    /// taken from inside a manifest.
    Explicit(PathBuf),
    Disabled,
}

struct Timer(Instant);

impl Timer {
    fn start() -> Self {
        Self(Instant::now())
    }

    fn ms(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1000.0
    }
}

impl LadderOutcome {
    fn step(
        &mut self,
        method: RecoveryMethod,
        outcome: StepOutcome,
        proof_level: ProofLevel,
        timer: &Timer,
        detail: impl Into<String>,
    ) {
        if outcome.leaves_incomplete() {
            self.incomplete = true;
        }
        if !outcome.ran_to_completion() {
            self.exhausted = false;
        }
        self.trace.push(RecoveryStep {
            method,
            outcome,
            proof_level,
            duration_ms: timer.ms(),
            detail: detail.into(),
        });
    }

    fn skip_remaining(&mut self, from: RecoveryMethod) {
        const ORDER: [RecoveryMethod; 6] = [
            RecoveryMethod::EmbeddedManifest,
            RecoveryMethod::SidecarManifest,
            RecoveryMethod::ContentHashLookup,
            RecoveryMethod::DecodedAudioHashLookup,
            RecoveryMethod::GenomarkRecovery,
            RecoveryMethod::FingerprintSearch,
        ];
        let mut reached = false;
        for method in ORDER {
            if method == from {
                reached = true;
                continue;
            }
            if !reached {
                continue;
            }
            self.exhausted = false;
            self.trace.push(RecoveryStep {
                method,
                outcome: StepOutcome::Skipped,
                proof_level: ProofLevel::UnknownUnobserved,
                duration_ms: 0.0,
                detail: "a stronger rung produced a candidate".to_string(),
            });
        }
    }
}

/// Runs the ladder, strongest binding first.
pub fn run(inputs: &LadderInputs<'_>) -> Result<LadderOutcome, GenoTraceError> {
    let mut outcome = LadderOutcome {
        exhausted: true,
        ..LadderOutcome::default()
    };

    if rung_embedded(inputs, &mut outcome) {
        outcome.skip_remaining(RecoveryMethod::EmbeddedManifest);
        return Ok(outcome);
    }
    if rung_sidecar(inputs, &mut outcome) {
        outcome.skip_remaining(RecoveryMethod::SidecarManifest);
        return Ok(outcome);
    }
    if rung_content_hash(inputs, &mut outcome) {
        outcome.skip_remaining(RecoveryMethod::ContentHashLookup);
        return Ok(outcome);
    }
    if rung_decoded_audio_hash(inputs, &mut outcome) {
        outcome.skip_remaining(RecoveryMethod::DecodedAudioHashLookup);
        return Ok(outcome);
    }
    if rung_genomark(inputs, &mut outcome) {
        outcome.skip_remaining(RecoveryMethod::GenomarkRecovery);
        return Ok(outcome);
    }
    rung_fingerprint(inputs, &mut outcome)?;
    Ok(outcome)
}

/// Feeds admitted bytes into the outcome. Returns true when the ladder must stop descending.
fn accept(
    outcome: &mut LadderOutcome,
    method: RecoveryMethod,
    bytes: &[u8],
    timer: &Timer,
) -> bool {
    if bytes.len() > MAX_MANIFEST_BYTES {
        outcome.step(
            method,
            StepOutcome::Degraded,
            ProofLevel::UnknownUnobserved,
            timer,
            format!(
                "manifest is {} bytes, over the admission limit",
                bytes.len()
            ),
        );
        return false;
    }
    match admit(bytes) {
        Admission::Admitted { manifest } => {
            outcome.step(
                method,
                StepOutcome::Hit,
                ProofLevel::DirectlyObserved,
                timer,
                format!("admitted {} manifest", manifest.schema().id()),
            );
            outcome.candidate = Some(Candidate { method, manifest });
            true
        }
        Admission::Rejected {
            rejection,
            signature,
            code,
            detail,
            findings,
        } => {
            outcome.diagnostics.extend(findings);
            outcome.step(
                method,
                StepOutcome::Degraded,
                ProofLevel::UserDeclared,
                timer,
                format!("{code}: {detail}"),
            );
            outcome.rejected = Some(match rejection {
                Some(rejection) => Rejected::Integrity {
                    method,
                    rejection,
                    detail,
                },
                None => Rejected::Signature {
                    method,
                    signature,
                    detail,
                },
            });
            true
        }
        // Not a candidate: the ladder steps over it and records why.
        Admission::Unparseable { code, detail } => {
            outcome.diagnostics.push(Finding::warning(
                "embedded_unparseable",
                "$",
                format!("{code}: {detail}"),
            ));
            outcome.step(
                method,
                StepOutcome::Miss,
                ProofLevel::UnknownUnobserved,
                timer,
                format!("provenance slot is not a readable manifest: {code}"),
            );
            false
        }
    }
}

fn rung_embedded(inputs: &LadderInputs<'_>, outcome: &mut LadderOutcome) -> bool {
    let timer = Timer::start();
    record_c2pa_diagnostics(inputs, outcome);

    match find_embedded(inputs.ingested.bytes(), inputs.ingested.container()) {
        EmbeddedOutcome::Found(bytes) => {
            accept(outcome, RecoveryMethod::EmbeddedManifest, &bytes, &timer)
        }
        EmbeddedOutcome::Absent => {
            outcome.step(
                RecoveryMethod::EmbeddedManifest,
                StepOutcome::Miss,
                ProofLevel::UnknownUnobserved,
                &timer,
                format!(
                    "no provenance slot in the {} container",
                    inputs.ingested.container().as_str()
                ),
            );
            false
        }
        EmbeddedOutcome::Unparseable(reason) => {
            outcome.diagnostics.push(Finding::warning(
                "embedded_unparseable",
                "$",
                reason.to_string(),
            ));
            outcome.step(
                RecoveryMethod::EmbeddedManifest,
                StepOutcome::Miss,
                ProofLevel::UnknownUnobserved,
                &timer,
                reason.to_string(),
            );
            false
        }
        EmbeddedOutcome::Unsearched(reason) => {
            outcome.step(
                RecoveryMethod::EmbeddedManifest,
                StepOutcome::Skipped,
                ProofLevel::UnknownUnobserved,
                &timer,
                reason.to_string(),
            );
            false
        }
    }
}

/// A C2PA store is read for its hard binding and reported, never admitted.
///
/// COSE verification (ES256/PS256) is not implemented anywhere in this workspace, and a candidate
/// whose signature this build cannot check would enter the ladder as `untrusted` and block every
/// lower rung. Reporting the recomputed hard binding as a finding keeps the information without
/// letting an uncheckable signature drive a verdict.
fn record_c2pa_diagnostics(inputs: &LadderInputs<'_>, outcome: &mut LadderOutcome) {
    let store = match inputs.ingested.container() {
        crate::ingest::Container::Wav => C2paStore::from_wav(inputs.ingested.bytes()).ok(),
        _ => None,
    }
    .or_else(|| {
        let path = inputs.source_path?;
        let sidecar = sidecar_path_for(&path.display().to_string());
        // Bounded before the read, not after: a store this build would refuse is not worth pulling
        // into memory first.
        if std::fs::metadata(&sidecar).ok()?.len()
            > audio_provenance_manifest::MAX_STORE_BYTES as u64
        {
            return None;
        }
        let bytes = std::fs::read(&sidecar).ok()?;
        C2paStore::from_sidecar(&bytes).ok()
    });

    let Some(store) = store else {
        return;
    };
    let detail = match store.hard_binding() {
        Ok(binding) => match binding.evaluate(inputs.ingested.bytes()) {
            Ok(BindingOutcome::Match) => "c2pa hard binding recomputes over this file".to_string(),
            Ok(BindingOutcome::Mismatch) => {
                "c2pa hard binding does not recompute over this file".to_string()
            }
            Ok(BindingOutcome::Uncoverable) => "c2pa hard binding is uncoverable".to_string(),
            Err(error) => format!("c2pa hard binding could not be evaluated: {error}"),
        },
        Err(error) => format!("c2pa store carries no readable hard binding: {error}"),
    };
    outcome.diagnostics.push(Finding::warning(
        "c2pa_cose_verification_unsupported",
        "$.c2pa",
        format!("{detail}; COSE signature verification is not implemented in this build"),
    ));
}

fn sidecar_candidates(inputs: &LadderInputs<'_>) -> Vec<PathBuf> {
    let Some(path) = inputs.source_path else {
        return Vec::new();
    };
    match &inputs.sidecar {
        SidecarPolicy::Disabled => Vec::new(),
        SidecarPolicy::Explicit(explicit) => vec![explicit.clone()],
        // IMPORTANT: resolved against the input's REAL directory only. No path from inside a
        // manifest is ever followed; `daemon/verify.py`'s `_safe_path` is the precedent.
        SidecarPolicy::Conventional => crate::sidecar_paths(path),
    }
}

fn rung_sidecar(inputs: &LadderInputs<'_>, outcome: &mut LadderOutcome) -> bool {
    let timer = Timer::start();
    let paths = sidecar_candidates(inputs);
    if paths.is_empty() {
        outcome.step(
            RecoveryMethod::SidecarManifest,
            StepOutcome::Skipped,
            ProofLevel::UnknownUnobserved,
            &timer,
            match inputs.source_path {
                Some(_) => "sidecar lookup is disabled".to_string(),
                None => "input has no filesystem path to resolve a sidecar against".to_string(),
            },
        );
        return false;
    }

    for path in &paths {
        let Ok(metadata) = std::fs::metadata(path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        if metadata.len() > MAX_MANIFEST_BYTES as u64 {
            outcome.step(
                RecoveryMethod::SidecarManifest,
                StepOutcome::Degraded,
                ProofLevel::UnknownUnobserved,
                &timer,
                format!("{} is over the manifest admission limit", path.display()),
            );
            return false;
        }
        match std::fs::read(path) {
            Ok(bytes) => {
                return accept(outcome, RecoveryMethod::SidecarManifest, &bytes, &timer);
            }
            Err(error) => {
                outcome.step(
                    RecoveryMethod::SidecarManifest,
                    StepOutcome::Unavailable,
                    ProofLevel::UnknownUnobserved,
                    &timer,
                    format!("{} could not be read: {error}", path.display()),
                );
                return false;
            }
        }
    }

    outcome.step(
        RecoveryMethod::SidecarManifest,
        StepOutcome::Miss,
        ProofLevel::UnknownUnobserved,
        &timer,
        format!(
            "no sidecar at {}",
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );
    false
}

fn registry_or_skip<'a>(
    inputs: &LadderInputs<'a>,
    outcome: &mut LadderOutcome,
    method: RecoveryMethod,
    timer: &Timer,
) -> Option<&'a dyn RegistryBackend> {
    if inputs.offline {
        // `--offline` is a deliberate choice, so the rung is `skipped`, never `unavailable`. It
        // must not set `incomplete` on its own.
        outcome.step(
            method,
            StepOutcome::Skipped,
            ProofLevel::UnknownUnobserved,
            timer,
            "offline: registry rungs were not attempted".to_string(),
        );
        return None;
    }
    match inputs.registry {
        Some(registry) => Some(registry),
        None => {
            outcome.step(
                method,
                StepOutcome::Skipped,
                ProofLevel::UnknownUnobserved,
                timer,
                "no registry is configured".to_string(),
            );
            None
        }
    }
}

fn handle_lookup(
    outcome: &mut LadderOutcome,
    method: RecoveryMethod,
    timer: &Timer,
    lookup: Lookup<RegistryRecord>,
    miss_detail: String,
) -> bool {
    match lookup {
        Lookup::Found(record) => {
            let bytes = record.manifest_bytes().to_vec();
            accept(outcome, method, &bytes, timer)
        }
        Lookup::NotFound => {
            outcome.step(
                method,
                StepOutcome::Miss,
                ProofLevel::UnknownUnobserved,
                timer,
                miss_detail,
            );
            false
        }
        // An outage is not a provenance verdict. It marks the search incomplete and the ladder
        // keeps descending.
        Lookup::Unavailable(reason) => {
            outcome.step(
                method,
                StepOutcome::Unavailable,
                ProofLevel::UnknownUnobserved,
                timer,
                format!("{}: {}", reason.kind().as_str(), reason.detail()),
            );
            false
        }
    }
}

fn rung_content_hash(inputs: &LadderInputs<'_>, outcome: &mut LadderOutcome) -> bool {
    let timer = Timer::start();
    let method = RecoveryMethod::ContentHashLookup;
    let Some(registry) = registry_or_skip(inputs, outcome, method, &timer) else {
        return false;
    };
    let digest = inputs.ingested.content_sha256();
    let Ok(content_hash) = ContentHash::parse_hex(digest) else {
        outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::UnknownUnobserved,
            &timer,
            "content digest is not a readable registry key".to_string(),
        );
        return false;
    };
    let lookup = registry.lookup_by_content_hash(&content_hash);
    handle_lookup(
        outcome,
        method,
        &timer,
        lookup,
        format!("sha256 {digest} is not in the registry"),
    )
}

fn rung_decoded_audio_hash(inputs: &LadderInputs<'_>, outcome: &mut LadderOutcome) -> bool {
    let timer = Timer::start();
    let method = RecoveryMethod::DecodedAudioHashLookup;
    if inputs.offline {
        outcome.step(
            method,
            StepOutcome::Skipped,
            ProofLevel::UnknownUnobserved,
            &timer,
            "offline: registry rungs were not attempted".to_string(),
        );
        return false;
    }
    let Some(index) = inputs.decoded_audio_index else {
        // Structural absence, not a fault: the configured backend exposes no such index. Skipped,
        // so `incomplete` stays false and a stock backend does not report an outage forever.
        outcome.step(
            method,
            StepOutcome::Skipped,
            ProofLevel::UnknownUnobserved,
            &timer,
            "the configured registry exposes no decoded-audio-hash index".to_string(),
        );
        return false;
    };
    let digest = inputs.ingested.decoded_audio_sha256();
    let Ok(key) = ContentHash::parse_hex(&digest) else {
        outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::UnknownUnobserved,
            &timer,
            "decoded-audio digest is not a readable registry key".to_string(),
        );
        return false;
    };
    let lookup = index.lookup_by_decoded_audio_hash(&key);
    handle_lookup(
        outcome,
        method,
        &timer,
        lookup,
        "decoded audio digest is not in the registry".to_string(),
    )
}

/// Runs GenoMark detection once and caches it. Later stages reuse the result rather than paying the
/// search again.
pub fn detect(genomark: &GenoMark, audio: &AudioBuffer) -> Option<DetectionOutcome> {
    genomark.detect(audio).ok()
}

fn rung_genomark(inputs: &LadderInputs<'_>, outcome: &mut LadderOutcome) -> bool {
    let timer = Timer::start();
    let method = RecoveryMethod::GenomarkRecovery;

    let detection = detect(inputs.genomark, inputs.ingested.audio());
    outcome.detection = detection.clone();
    let Some(detection) = detection else {
        outcome.step(
            method,
            StepOutcome::Degraded,
            ProofLevel::UnknownUnobserved,
            &timer,
            "detector could not run on this audio".to_string(),
        );
        return false;
    };
    let Some(payload) = detection.payload() else {
        outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::UnknownUnobserved,
            &timer,
            format!(
                "no payload; {} sync candidates, {} CRC attempts",
                detection.sync_candidates(),
                detection.crc_attempts()
            ),
        );
        return false;
    };
    if detection.candidate_cap_reached() {
        outcome.diagnostics.push(Finding::warning(
            "genomark_candidate_cap_reached",
            "$.genomark",
            "peaks above the sync threshold were dropped unexamined; the recovery figure for this \
             input is a cap artifact"
                .to_string(),
        ));
    }
    if detection.namespace_mismatch() {
        outcome.diagnostics.push(Finding::warning(
            "genomark_namespace_mismatch",
            "$.genomark",
            format!(
                "accepted payload names namespace {}, not the {} this detector is keyed for",
                payload.namespace(),
                inputs.genomark.namespace()
            ),
        ));
    }

    let Some(registry) = registry_or_skip(inputs, outcome, method, &timer) else {
        return false;
    };
    let Ok(mark) = MarkId::new(
        payload.version(),
        payload.namespace(),
        payload.locator_bytes(),
    ) else {
        outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::UnknownUnobserved,
            &timer,
            "recovered payload is not a readable registry key".to_string(),
        );
        return false;
    };

    let matches = match registry.lookup_by_mark(&mark) {
        Lookup::Found(matches) => matches,
        Lookup::NotFound => {
            outcome.step(
                method,
                StepOutcome::Miss,
                ProofLevel::UnknownUnobserved,
                &timer,
                format!("mark {} is not registered", mark.to_hex()),
            );
            return false;
        }
        Lookup::Unavailable(reason) => {
            outcome.step(
                method,
                StepOutcome::Unavailable,
                ProofLevel::UnknownUnobserved,
                &timer,
                format!("{}: {}", reason.kind().as_str(), reason.detail()),
            );
            return false;
        }
    };

    let expected_locator = payload.locator_bytes();
    let mut survivors: Vec<(RecordId, Vec<u8>)> = Vec::new();
    for record in matches.iter() {
        // IMPORTANT: re-derived over the RECEIVED bytes, from the document's OWN declared signing
        // key and locator_salt. Never from the backend's mark_id(), which is the claim this check
        // exists to distrust, and never from a trust anchor, which is a later stage. A record with
        // no usable salt derives nothing, and that is a mismatch, not a pass.
        //
        // The declared key is UNVERIFIED here, because this runs before admission. That is safe:
        // deriving the locator under a key the document does not actually hold buys the attacker
        // only `signature_invalid`, since admission verifies the signature against that same
        // declared key moments later.
        let received = record.manifest_bytes();
        let derived = parse_signing_input(received)
            .ok()
            .as_ref()
            .and_then(locator_from_signed_manifest);
        if derived != Some(expected_locator) {
            outcome.step(
                method,
                StepOutcome::Degraded,
                ProofLevel::UnknownUnobserved,
                &timer,
                format!(
                    "record {} does not re-derive the payload locator",
                    record.record_id().to_hex()
                ),
            );
            outcome.rejected = Some(Rejected::Integrity {
                method,
                rejection: RejectionReason::LocatorMismatch,
                detail: format!(
                    "registry returned a manifest that does not derive locator {}",
                    hex::encode(expected_locator)
                ),
            });
            return true;
        }
        survivors.push((record.record_id(), received.to_vec()));
    }

    // The association here was recovered from the audio, so a soft-binding failure is a rejected
    // guess: the candidate is discarded and the ladder continues.
    let mut admitted: Vec<(RecordId, Vec<u8>)> = Vec::new();
    for (record_id, bytes) in survivors {
        match soft_binding_agrees(&bytes, payload.version(), payload.namespace()) {
            SoftAgreement::Disagrees(detail) => {
                outcome.discarded.push(DiscardedCandidate {
                    method,
                    record_digest: Some(record_id.to_hex()),
                    reason: "soft_binding_mismatch",
                    detail,
                });
            }
            SoftAgreement::AgreesOrUnavailable => admitted.push((record_id, bytes)),
        }
    }

    match admitted.len() {
        0 => {
            outcome.step(
                method,
                StepOutcome::Miss,
                ProofLevel::UnknownUnobserved,
                &timer,
                "every registered record for this mark describes different audio".to_string(),
            );
            false
        }
        1 => {
            let (_, bytes) = &admitted[0];
            let proof = match detection.class() {
                ConfidenceClass::Strong => ProofLevel::DirectlyObserved,
                ConfidenceClass::Single | ConfidenceClass::None => ProofLevel::Inferred,
            };
            let accepted = accept(outcome, method, bytes, &timer);
            if let Some(step) = outcome.trace.last_mut() {
                step.proof_level = proof;
                step.detail = format!(
                    "{}, {} blocks, class {}",
                    step.detail,
                    detection.blocks_accepted(),
                    detection.class().as_str()
                );
            }
            accepted
        }
        _ => {
            outcome.step(
                method,
                StepOutcome::Degraded,
                ProofLevel::UnknownUnobserved,
                &timer,
                format!(
                    "{} registered records survive disambiguation",
                    admitted.len()
                ),
            );
            outcome.rejected = Some(Rejected::Integrity {
                method,
                rejection: RejectionReason::AmbiguousBinding,
                detail: admitted
                    .iter()
                    .map(|(record_id, _)| record_id.to_hex())
                    .collect::<Vec<_>>()
                    .join(", "),
            });
            true
        }
    }
}

enum SoftAgreement {
    AgreesOrUnavailable,
    Disagrees(String),
}

/// Stage 3.2 for the mark: does the manifest's declared mark describe the payload we recovered?
///
/// This is the watermark-copy check. A payload lifted from one track and pasted into another still
/// resolves a registry key; what it cannot do is agree with the record the key names.
fn soft_binding_agrees(bytes: &[u8], version: u8, namespace: u8) -> SoftAgreement {
    let Ok(parsed) = audio_provenance_core::parse_signing_input(bytes) else {
        return SoftAgreement::AgreesOrUnavailable;
    };
    let Some(mark) = parsed.get("mark").and_then(|value| value.as_object()) else {
        return SoftAgreement::AgreesOrUnavailable;
    };
    let declared_version = mark.get("version").and_then(serde_json::Value::as_u64);
    let declared_namespace = mark.get("namespace").and_then(serde_json::Value::as_u64);
    match (declared_version, declared_namespace) {
        (Some(declared_version), Some(declared_namespace)) => {
            if declared_version == u64::from(version) && declared_namespace == u64::from(namespace)
            {
                SoftAgreement::AgreesOrUnavailable
            } else {
                SoftAgreement::Disagrees(format!(
                    "record declares mark v{declared_version} ns{declared_namespace}, audio \
                     carries v{version} ns{namespace}"
                ))
            }
        }
        _ => SoftAgreement::AgreesOrUnavailable,
    }
}

fn rung_fingerprint(
    inputs: &LadderInputs<'_>,
    outcome: &mut LadderOutcome,
) -> Result<(), GenoTraceError> {
    let timer = Timer::start();
    let method = RecoveryMethod::FingerprintSearch;

    let Some(index) = inputs.fingerprint_index else {
        outcome.step(
            method,
            StepOutcome::Skipped,
            ProofLevel::Inferred,
            &timer,
            "no fingerprint index is configured".to_string(),
        );
        return Ok(());
    };

    let query = FingerprintQuery::from_audio(inputs.ingested.audio())?;
    let search = crate::fingerprint::search(index, &query, inputs.score_limits)?;
    outcome.fingerprint = Some(search.clone());

    if search.too_short {
        outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::Inferred,
            &timer,
            format!(
                "query is {:.2} s, under the {:.1} s a fingerprint answer needs",
                query.seconds(),
                crate::fingerprint::MIN_QUERY_SECONDS
            ),
        );
        return Ok(());
    }
    if search.budget_exceeded {
        outcome.diagnostics.push(Finding::warning(
            "fingerprint_budget_exceeded",
            "$.fingerprint",
            format!(
                "{} postings scanned before the budget stopped the search",
                search.postings_scanned
            ),
        ));
        // Aborted, not degraded silently: a truncated scan understates every score it did not
        // finish, so the rung could not run.
        outcome.step(
            method,
            StepOutcome::Unavailable,
            ProofLevel::Inferred,
            &timer,
            "posting budget exceeded".to_string(),
        );
        return Ok(());
    }

    let Some(hit) = search.hit.clone() else {
        outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::Inferred,
            &timer,
            format!(
                "{} query hashes matched no track above the alignment gates",
                search.query_hashes
            ),
        );
        return Ok(());
    };
    if search.ambiguous {
        outcome.step(
            method,
            StepOutcome::Degraded,
            ProofLevel::Inferred,
            &timer,
            "two tracks cleared every alignment gate".to_string(),
        );
        outcome.rejected = Some(Rejected::Integrity {
            method,
            rejection: RejectionReason::AmbiguousBinding,
            detail: "fingerprint search resolved to more than one recording".to_string(),
        });
        return Ok(());
    }

    outcome.diagnostics.push(Finding::info(
        "fingerprint_candidate",
        "$.fingerprint",
        format!(
            "record {} aligns at offset {} frames, peak {}, coherence {:.4}, rate {:.2}",
            hit.record.to_hex(),
            hit.alignment.peak_offset_frames,
            hit.alignment.peak,
            hit.alignment.coherence_ratio,
            hit.rate
        ),
    ));

    if inputs.inferred == InferredAssociationPolicy::DiagnosticsOnly {
        // The default policy emits no candidate. The finding above records what was seen; the
        // terminal status stays `not_found`.
        outcome.step(
            method,
            StepOutcome::Skipped,
            ProofLevel::Inferred,
            &timer,
            "a candidate was found but inferred association is not accepted".to_string(),
        );
        return Ok(());
    }

    let Some(registry) = registry_or_skip(inputs, outcome, method, &timer) else {
        return Ok(());
    };
    match registry.fetch(&hit.record) {
        Lookup::Found(record) => {
            let bytes = record.manifest_bytes().to_vec();
            accept(outcome, method, &bytes, &timer);
            if let Some(step) = outcome.trace.last_mut() {
                step.proof_level = ProofLevel::Inferred;
            }
        }
        Lookup::NotFound => outcome.step(
            method,
            StepOutcome::Miss,
            ProofLevel::Inferred,
            &timer,
            format!("record {} is indexed but absent", hit.record.to_hex()),
        ),
        Lookup::Unavailable(reason) => outcome.step(
            method,
            StepOutcome::Unavailable,
            ProofLevel::Inferred,
            &timer,
            format!("{}: {}", reason.kind().as_str(), reason.detail()),
        ),
    }
    Ok(())
}

/// Stage 3: recompute the manifest's bindings against the audio actually presented.
/// Everything Stage 3 reads. A struct rather than a parameter list because the corroboration path
/// needs most of it too, and threading eight positional arguments through two functions is how a
/// detector and a null-test table end up swapped.
#[derive(Debug)]
pub struct BindingInputs<'a> {
    pub manifest: &'a Manifest,
    pub ingested: &'a Ingested,
    pub genomark: &'a GenoMark,
    /// The GenoMark rung's detection, when the ladder reached it.
    pub detection: Option<&'a DetectionOutcome>,
    pub fingerprint: Option<&'a FingerprintSearch>,
    pub null_test: &'a NullTestTable,
    pub score_limits: ScoreLimits,
    /// The GenoMark block-coverage guard. Route 3(c) needs it as well as the class projection does:
    /// the guard is what fails a marked insert spliced into unmarked audio, and the fingerprint's
    /// own floor cannot, because a splice is genuinely part of the signed work.
    pub soft_binding_threshold: f64,
}

pub fn evaluate_binding(
    inputs: &BindingInputs<'_>,
    diagnostics: &mut Vec<Finding>,
) -> BindingEvaluation {
    let BindingInputs {
        manifest,
        ingested,
        fingerprint,
        null_test,
        ..
    } = *inputs;

    // PERF: the hard binding is two digest comparisons and the detector is a full pass over the
    // audio, so the cheap answer is taken first. An exact hard binding must never pay for a
    // detection whose result it would discard.
    if let Some(binding) = manifest.hard_binding() {
        match binding.evaluate_content(ingested.content_sha256()) {
            BindingOutcome::Match => return BindingEvaluation::HardExactContent,
            BindingOutcome::Mismatch => {
                if binding.evaluate_decoded_audio(&ingested.decoded_audio_sha256())
                    == BindingOutcome::Match
                {
                    return BindingEvaluation::HardExactDecodedAudioOnly;
                }
                return corroborate(inputs, diagnostics, binding.content_sha256().to_string());
            }
            BindingOutcome::Uncoverable => {}
        }
    }

    // A manifest recovered above the GenoMark rung carries no detection, and this path cannot
    // honestly call the soft evidence missing without one.
    let late;
    let detection = match inputs.detection {
        Some(outcome) => Some(outcome),
        None => {
            late = detect(inputs.genomark, ingested.audio());
            late.as_ref()
        }
    };

    if let Some((detection, payload)) =
        detection.and_then(|detection| detection.payload().map(|payload| (detection, payload)))
    {
        // IMPORTANT: the same term route 3(c) applies at `corroborate`. A CRC-valid payload is
        // mintable by anyone, because namespace 0's profile key is published, and a locator is a
        // public registry key; so a mark that does not name THIS record's locator is evidence about
        // some other record and may not stand in for the hard binding this one never declared.
        // Without it, any record carrying no hard binding is verified by any marked audio at all.
        if manifest.mark_locator() == Some(payload.locator_bytes()) {
            let (blocks_present, score) =
                mark_coverage(detection.blocks_accepted(), ingested.audio());
            let blocks_agreeing = detection.blocks_accepted();
            let distribution = mark_distribution(detection, ingested.audio());
            if !distribution.consistent {
                diagnostics.push(Finding::warning(
                    "recovered_mark_distribution_disagrees",
                    "$.mark",
                    format!(
                        "{blocks_agreeing} accepted blocks were clustered rather than distributed \
                         across the presented recording: maximum uncovered span {:.2} s exceeds \
                         the {:.2} s block-distribution allowance",
                        distribution.max_uncovered_gap_seconds, distribution.allowance_seconds,
                    ),
                ));
                return BindingEvaluation::NoEvidence;
            }
            let confidence = match detection.class() {
                ConfidenceClass::Strong => MarkConfidence::Strong,
                ConfidenceClass::Single | ConfidenceClass::None => MarkConfidence::Single,
            };
            return match null_test.rate_for(genomark::ALGORITHM_ID) {
                Some(false_positive_rate) => BindingEvaluation::SoftMark {
                    confidence,
                    score,
                    blocks_agreeing,
                    blocks_present,
                    false_positive_rate,
                },
                None => BindingEvaluation::SoftMarkUnpriced {
                    confidence,
                    score,
                    blocks_agreeing,
                    blocks_present,
                },
            };
        }
        diagnostics.push(Finding::warning(
            "recovered_mark_names_another_record",
            "$.mark",
            "a GenoMark payload decoded from this audio, but its locator is not the one this \
             record answers to, so it is not evidence about this record",
        ));
    }

    if let Some(hit) = fingerprint.and_then(|search| search.hit.as_ref()) {
        return BindingEvaluation::SoftFingerprint {
            score: MatchScore::soft(hit.alignment.coherence_ratio),
            coherence_ratio: hit.alignment.coherence_ratio,
            false_positive_rate: null_test.rate_for(crate::fingerprint::ALGORITHM_ID),
        };
    }

    BindingEvaluation::NoEvidence
}

/// Stage 3.2's route 3(c): the signed digest failed, so ask whether this is nonetheless the same
/// work.
///
/// THREE terms are required and none is sufficient. The payload must CRC-pass and must name this
/// record's own locator, which is what stops an unrelated marked file being presented alongside
/// somebody else's sidecar. The record's signed reference constellation must affirm the audio. And
/// the mark must cover the audio to the same block-coverage guard every other soft binding clears.
///
/// The third term is what a splice fails. Nine marked seconds pasted into eleven unmarked ones is
/// genuinely part of the signed work, so the constellation aligns across it and the alignment floor
/// is the wrong instrument; the mark covering one block in two is the fact that says the rest of
/// this file is not the recording that was signed. A payload that decodes while the constellation
/// disagrees is the watermark-copy attack. Both are `changed`, which is the true statement about
/// each.
fn corroborate(
    inputs: &BindingInputs<'_>,
    diagnostics: &mut Vec<Finding>,
    expected: String,
) -> BindingEvaluation {
    let BindingInputs {
        manifest,
        ingested,
        null_test,
        score_limits,
        soft_binding_threshold,
        ..
    } = *inputs;
    let mismatch = || BindingEvaluation::HardMismatch {
        expected: expected.clone(),
    };

    let Some(declared) = manifest.fingerprint() else {
        return mismatch();
    };
    let reference = match ReferenceFingerprint::from_manifest_fingerprint(declared) {
        Ok(Some(reference)) => reference,
        // A descriptor under another algorithm is evidence this build cannot read, and a malformed
        // one is a defective record. Neither is evidence about the audio, so neither may affirm.
        Ok(None) => {
            diagnostics.push(Finding::warning(
                "reference_fingerprint_unreadable",
                "$.fingerprint",
                format!(
                    "the record declares a {} fingerprint, which this build cannot compare",
                    declared.algorithm()
                ),
            ));
            return mismatch();
        }
        Err(error) => {
            diagnostics.push(Finding::warning(
                "reference_fingerprint_malformed",
                "$.fingerprint",
                error.to_string(),
            ));
            return mismatch();
        }
    };

    // PERF: only now, with a readable reference in hand, is a full detection pass worth its cost.
    // A record that declares no comparable constellation can never take route 3(c), so detecting
    // before this point would buy an answer nothing could use.
    let late;
    let detection = match inputs.detection {
        Some(outcome) => Some(outcome),
        None => {
            late = detect(inputs.genomark, ingested.audio());
            late.as_ref()
        }
    };
    let Some((detection, payload)) =
        detection.and_then(|detection| detection.payload().map(|payload| (detection, payload)))
    else {
        return mismatch();
    };
    if manifest.mark_locator() != Some(payload.locator_bytes()) {
        diagnostics.push(Finding::warning(
            "recovered_mark_names_another_record",
            "$.mark",
            "a GenoMark payload decoded from this audio, but its locator is not the one this              record answers to",
        ));
        return mismatch();
    }

    let query = match FingerprintQuery::from_audio(ingested.audio()) {
        Ok(query) => query,
        Err(error) => {
            diagnostics.push(Finding::warning(
                "reference_fingerprint_uncomparable",
                "$.fingerprint",
                error.to_string(),
            ));
            return mismatch();
        }
    };
    let affirmation = match crate::fingerprint::compare(&reference, &query, score_limits) {
        Ok(affirmation) => affirmation,
        Err(error) => {
            diagnostics.push(Finding::warning(
                "reference_fingerprint_uncomparable",
                "$.fingerprint",
                error.to_string(),
            ));
            return mismatch();
        }
    };
    if !affirmation.affirmed {
        diagnostics.push(Finding::warning(
            "reference_constellation_disagrees",
            "$.fingerprint",
            format!(
                "a GenoMark payload for this record decoded, but the signed reference \
                 constellation does not describe this recording: {}/{} expected local regions \
                 aligned ({:.0}%), maximum unexplained gap {:.2} s, {} offset discontinuities, \
                 duration consistent {}, peak {} against {} outside it, span {:.2} s",
                affirmation.regions_covered,
                affirmation.regions_expected,
                affirmation.coverage * 100.0,
                affirmation.max_unexplained_gap_seconds,
                affirmation.offset_discontinuities,
                affirmation.duration_consistent,
                affirmation.peak,
                affirmation.best_outside_peak,
                affirmation.aligned_span_seconds,
            ),
        ));
        return mismatch();
    }

    let blocks_agreeing = detection.blocks_accepted();
    let (blocks_present, mark_score) = mark_coverage(blocks_agreeing, ingested.audio());
    let distribution = mark_distribution(detection, ingested.audio());
    if !distribution.consistent {
        diagnostics.push(Finding::warning(
            "recovered_mark_distribution_disagrees",
            "$.mark",
            format!(
                "the signed duration and reference constellation align, but the accepted mark \
                 blocks are clustered: maximum uncovered span {:.2} s exceeds the {:.2} s \
                 block-distribution allowance",
                distribution.max_uncovered_gap_seconds, distribution.allowance_seconds,
            ),
        ));
        return mismatch();
    }
    if mark_score.value() < soft_binding_threshold {
        diagnostics.push(Finding::warning(
            "recovered_mark_covers_too_little",
            "$.mark",
            format!(
                "the signed reference constellation aligns, but the mark covers only                  {blocks_agreeing} of the {blocks_present} whole blocks in this audio, under the                  {soft_binding_threshold:.2} guard: part of this file was not marked with this                  record"
            ),
        ));
        return mismatch();
    }
    let score = MatchScore::soft(affirmation.coverage);
    match null_test.rate_for(genomark::ALGORITHM_ID) {
        Some(false_positive_rate) => BindingEvaluation::HardMismatchSoftAffirmed {
            expected,
            score,
            coverage: affirmation.coverage,
            blocks_agreeing,
            blocks_present,
            false_positive_rate,
        },
        None => BindingEvaluation::HardMismatchSoftAffirmedUnpriced {
            expected,
            score,
            coverage: affirmation.coverage,
            blocks_agreeing,
            blocks_present,
        },
    }
}

/// The mark's coverage over this audio: whole blocks present, and agreeing over present.
///
/// IMPORTANT: the denominator is the whole-block count OR the number of blocks that actually
/// decoded, whichever is larger. A block the detector accepted is a whole block present by
/// definition, and the floor of a duration can sit one below it at a boundary; taking the floor
/// alone would score a genuinely short marked file at zero and send the coverage guard's own worked
/// example to `untrusted`. The max never lowers the true denominator, so it cannot inflate a score.
pub fn mark_coverage(blocks_agreeing: usize, audio: &AudioBuffer) -> (usize, MatchScore) {
    let capabilities = genomark::Capabilities::at(audio.sample_rate());
    let whole = if capabilities.block_seconds > 0.0 {
        (audio.duration_seconds() / capabilities.block_seconds).floor() as usize
    } else {
        0
    };
    let blocks_present = whole.max(blocks_agreeing);
    if blocks_present == 0 {
        return (0, MatchScore::zero());
    }
    (
        blocks_present,
        MatchScore::soft(blocks_agreeing as f64 / blocks_present as f64),
    )
}

#[derive(Debug, Clone, Copy)]
struct MarkDistribution {
    max_uncovered_gap_seconds: f64,
    allowance_seconds: f64,
    consistent: bool,
}

fn mark_distribution(detection: &DetectionOutcome, audio: &AudioBuffer) -> MarkDistribution {
    let block_seconds = genomark::Capabilities::at(audio.sample_rate()).block_seconds;
    mark_distribution_from_spans(
        detection.accepted_spans_seconds(),
        audio.duration_seconds(),
        block_seconds,
    )
}

/// A crop may begin and end inside a watermark block, so one block plus a quarter-block STFT/rate
/// allowance may be uncovered at either edge. Two or more missing interior blocks cannot hide
/// behind a good total count: their uncovered interval exceeds this bound.
fn mark_distribution_from_spans(
    spans: &[(f64, f64)],
    duration_seconds: f64,
    block_seconds: f64,
) -> MarkDistribution {
    let allowance_seconds = block_seconds * 1.25;
    if spans.is_empty() || duration_seconds <= 0.0 || block_seconds <= 0.0 {
        return MarkDistribution {
            max_uncovered_gap_seconds: duration_seconds.max(0.0),
            allowance_seconds,
            consistent: false,
        };
    }
    let mut spans = spans.to_vec();
    spans.sort_by(|left, right| left.0.total_cmp(&right.0));
    let mut cursor = 0.0f64;
    let mut max_gap = 0.0f64;
    for (start, end) in spans {
        let start = start.clamp(0.0, duration_seconds);
        let end = end.clamp(start, duration_seconds);
        max_gap = max_gap.max((start - cursor).max(0.0));
        cursor = cursor.max(end);
    }
    max_gap = max_gap.max((duration_seconds - cursor).max(0.0));
    MarkDistribution {
        max_uncovered_gap_seconds: max_gap,
        allowance_seconds,
        consistent: max_gap <= allowance_seconds,
    }
}

#[cfg(test)]
mod mark_distribution_tests {
    use super::*;

    #[test]
    fn block_count_and_block_distribution_are_independent_gates() {
        let block = 10.0;
        let distributed =
            mark_distribution_from_spans(&[(0.2, 9.8), (10.1, 19.9), (20.0, 29.7)], 30.0, block);
        assert!(distributed.consistent, "{distributed:?}");

        // The same three accepted blocks clustered at the head of a five-block presentation could
        // clear a count-only threshold of 3/5 under a relaxed policy. Distribution still rejects.
        let clustered =
            mark_distribution_from_spans(&[(0.2, 9.8), (10.1, 19.9), (20.0, 29.7)], 50.0, block);
        assert!(!clustered.consistent, "{clustered:?}");
        assert!(clustered.max_uncovered_gap_seconds > clustered.allowance_seconds);
    }
}
