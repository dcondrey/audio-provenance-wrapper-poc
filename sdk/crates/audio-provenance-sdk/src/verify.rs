//! The consumer surface: options a caller owns, and the four entry points that read them.

use std::path::Path;
use std::sync::Arc;

use audio_provenance_core::VerificationStatus;
use audio_provenance_registry::config::ProcessEnv;
use audio_provenance_registry::{RegistryBackend, RegistryResolver};
use apw_watermark::Watermark;
use apw_trace::{
    DEFAULT_SOFT_BINDING_THRESHOLD, FileFingerprintIndex, FileTrustStore, FingerprintIndex,
    InferredAssociationPolicy, IngestLimits, InspectReport, NullTestTable, RecoveryMethod,
    SidecarPolicy, StepOutcome, TrustStore, VerifyResult,
};

use crate::error::SdkError;

/// Everything a verification may be pointed at, owned.
///
/// Trace's own options borrow, which is right for a hot loop and wrong for a caller that just
/// resolved a registry name and a trust-store path. This owns those resources and lends them to
/// Trace for the length of one call.
#[derive(Debug)]
pub struct VerifyOptions {
    registry: Option<Arc<dyn RegistryBackend>>,
    trust_store: Option<Box<dyn TrustStore>>,
    fingerprint_index: Option<Box<dyn FingerprintIndex>>,
    null_test: NullTestTable,
    apw_watermark: Watermark,
    sidecar: SidecarPolicy,
    offline: bool,
    soft_binding_threshold: f64,
    inferred: InferredAssociationPolicy,
    limits: IngestLimits,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            registry: None,
            trust_store: None,
            fingerprint_index: None,
            null_test: NullTestTable::empty(),
            apw_watermark: Watermark::public(),
            sidecar: SidecarPolicy::Conventional,
            offline: false,
            soft_binding_threshold: DEFAULT_SOFT_BINDING_THRESHOLD,
            inferred: InferredAssociationPolicy::DiagnosticsOnly,
            limits: IngestLimits::default(),
        }
    }
}

impl VerifyOptions {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_registry(mut self, registry: Arc<dyn RegistryBackend>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Resolves a configured NAME (`public`, `local`, anything in the config file) to a backend.
    ///
    /// There is no fallback endpoint: an unconfigured name is an error naming the variable and the
    /// file to set, because a guessed URL presented as a default is a guess presented as a fact.
    pub fn with_registry_named(
        self,
        name: &str,
        config_path: Option<&Path>,
    ) -> Result<Self, SdkError> {
        let resolver = RegistryResolver::load(config_path, &ProcessEnv)?;
        Ok(self.with_registry(resolver.resolve(name)?))
    }

    #[must_use]
    pub fn with_trust_store(mut self, store: Box<dyn TrustStore>) -> Self {
        self.trust_store = Some(store);
        self
    }

    pub fn with_trust_store_path(self, path: &Path) -> Result<Self, SdkError> {
        Ok(self.with_trust_store(Box::new(FileTrustStore::load(path)?)))
    }

    #[must_use]
    pub fn with_fingerprint_index(mut self, index: Box<dyn FingerprintIndex>) -> Self {
        self.fingerprint_index = Some(index);
        self
    }

    pub fn with_fingerprint_index_path(self, path: &Path) -> Result<Self, SdkError> {
        Ok(self.with_fingerprint_index(Box::new(FileFingerprintIndex::open(path)?)))
    }

    /// Folds a `audio-provenance-bench` null-test report in. Without one, no soft binding reaches
    /// `verified`: the false-positive rate that a soft `verified` must publish has no honest value
    /// until the bench has actually run.
    pub fn with_null_test_report(mut self, path: &Path) -> Result<Self, SdkError> {
        let bytes = std::fs::read(path).map_err(|source| SdkError::io(path, source))?;
        self.null_test
            .merge(NullTestTable::from_bench_report(&bytes)?);
        Ok(self)
    }

    #[must_use]
    pub fn with_null_test(mut self, table: NullTestTable) -> Self {
        self.null_test = table;
        self
    }

    /// Selects a keyed Watermark profile. The default is namespace 0, whose profile key is published
    /// and whose mark is therefore recovery, not tamper resistance.
    #[must_use]
    pub fn with_apw_watermark(mut self, apw_watermark: Watermark) -> Self {
        self.apw_watermark = apw_watermark;
        self
    }

    #[must_use]
    pub fn with_sidecar(mut self, policy: SidecarPolicy) -> Self {
        self.sidecar = policy;
        self
    }

    /// Registry rungs become `skipped`, never `unavailable`, so offline alone never fails a
    /// verification.
    #[must_use]
    pub const fn offline(mut self, offline: bool) -> Self {
        self.offline = offline;
        self
    }

    /// Lets the fingerprint rung emit a candidate. It can still never produce `verified`.
    #[must_use]
    pub const fn accept_inferred_association(mut self, accept: bool) -> Self {
        self.inferred = if accept {
            InferredAssociationPolicy::EmitCandidate
        } else {
            InferredAssociationPolicy::DiagnosticsOnly
        };
        self
    }

    pub fn with_soft_binding_threshold(mut self, threshold: f64) -> Result<Self, SdkError> {
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(SdkError::InvalidOption {
                option: "soft_binding_threshold",
                reason: "must be a number in [0, 1]".to_string(),
            });
        }
        self.soft_binding_threshold = threshold;
        Ok(self)
    }

    #[must_use]
    pub const fn with_limits(mut self, limits: IngestLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn registry(&self) -> Option<&dyn RegistryBackend> {
        self.registry.as_deref()
    }

    fn borrowed(&self) -> Result<apw_trace::VerifyOptions<'_>, SdkError> {
        let mut options = apw_trace::VerifyOptions::new()
            .with_apw_watermark(&self.apw_watermark)
            .with_null_test(&self.null_test)
            .with_sidecar(self.sidecar.clone())
            .offline(self.offline)
            .with_inferred_association(self.inferred)
            .with_limits(self.limits)
            .with_soft_binding_threshold(self.soft_binding_threshold)?;
        if let Some(registry) = &self.registry {
            options = options.with_registry(registry.as_ref());
        }
        if let Some(store) = &self.trust_store {
            options = options.with_trust_store(store.as_ref());
        }
        if let Some(index) = &self.fingerprint_index {
            options = options.with_fingerprint_index(index.as_ref());
        }
        Ok(options)
    }

    pub fn verify(&self, path: &Path) -> Result<VerifyResult, SdkError> {
        verify(path, self)
    }

    pub fn verify_bytes(&self, bytes: Vec<u8>) -> Result<VerifyResult, SdkError> {
        verify_bytes(bytes, self)
    }

    pub fn inspect(&self, path: &Path) -> Result<InspectReport, SdkError> {
        inspect(path, self)
    }
}

/// Verifies a file on disk.
///
/// Returns `Err` only for a caller error or for a search that could not finish. Every provenance
/// condition is one of the four statuses on the `Ok` side.
pub fn verify(path: &Path, options: &VerifyOptions) -> Result<VerifyResult, SdkError> {
    settled(apw_trace::verify(path, &options.borrowed()?)?)
}

/// Verifies bytes already in hand. Sidecar rungs cannot run: there is no real directory to resolve
/// them against, and no path from inside a manifest is ever followed.
pub fn verify_bytes(bytes: Vec<u8>, options: &VerifyOptions) -> Result<VerifyResult, SdkError> {
    settled(apw_trace::verify_bytes(bytes, &options.borrowed()?)?)
}

/// Reports what Trace found, with no verdict and no trust evaluation.
///
/// An incomplete search is reported here rather than raised: there is no verdict for an outage to
/// contaminate.
pub fn inspect(path: &Path, options: &VerifyOptions) -> Result<InspectReport, SdkError> {
    Ok(apw_trace::inspect(path, &options.borrowed()?)?)
}

/// Rungs whose only `unavailable` outcome is a registry that could not answer.
const fn registry_backed(method: RecoveryMethod) -> bool {
    matches!(
        method,
        RecoveryMethod::ContentHashLookup
            | RecoveryMethod::DecodedAudioHashLookup
            | RecoveryMethod::WatermarkRecovery
    )
}

/// Refuses to return `not_found` from a search that did not finish.
///
/// `not_found` means "nothing was recovered", and a caller reads it as a fact about the work. When
/// a rung could not run, that reading is unearned, so the incompleteness is raised instead of being
/// buried in a boolean the caller may not check. A definite verdict is unaffected: `verified`,
/// `changed` and `untrusted` all rest on a candidate that was actually admitted, and they are
/// returned with `incomplete` set if a lower rung faulted.
fn settled(result: VerifyResult) -> Result<VerifyResult, SdkError> {
    if result.status != VerificationStatus::NotFound || !result.incomplete {
        return Ok(result);
    }
    let faulted = result
        .trace
        .iter()
        .find(|step| step.outcome == StepOutcome::Unavailable);
    match faulted {
        Some(step) if registry_backed(step.method) => Err(SdkError::RegistryUnavailable {
            method: step.method.as_str(),
            detail: step.detail.clone(),
        }),
        Some(step) => Err(SdkError::RecoveryIncomplete {
            method: step.method.as_str(),
            detail: step.detail.clone(),
        }),
        // `incomplete` without an unavailable step is the ladder itself failing to run, which
        // Trace records as a `recovery_rung_failed` diagnostic.
        None => Err(SdkError::RecoveryIncomplete {
            method: "recovery_ladder",
            detail: result
                .recovery
                .diagnostics
                .iter()
                .map(|finding| finding.message.clone())
                .collect::<Vec<_>>()
                .join("; "),
        }),
    }
}
