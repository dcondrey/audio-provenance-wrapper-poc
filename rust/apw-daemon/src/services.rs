use std::path::{Path, PathBuf};

use apw_core::{IngredientEvidence, ProofLevel, StemEvidence, PROOF_LEVEL_KEY};
use serde_json::{json, Value};

/// `audio_association.METHOD` / `METHOD_VERSION`: the association record names the
/// comparison that produced it, and a verifier re-runs the same method version.
pub const ASSOCIATION_METHOD: &str = "routed_feature_sequence_offset_search";
pub const ASSOCIATION_METHOD_VERSION: &str = "2.0.0";

/// The stem/export comparison seam, implemented by the audio crate.
pub trait ExportAssociator: Send + Sync {
    fn associate(&self, export_path: &Path, routed_features: &[Value]) -> Value;
}

/// IMPORTANT: an unavailable comparison is not evidence that routed audio was
/// absent from the export. The record says so in its own `limitations`, and the
/// proof level stays `unknown_unobserved`.
pub fn unavailable_association(reason: &str) -> Value {
    json!({
        "status": "unavailable",
        "method": ASSOCIATION_METHOD,
        "method_version": ASSOCIATION_METHOD_VERSION,
        "confidence": Value::Null,
        "matched_coverage": 0.0,
        "routed_coverage": 0.0,
        "matched_window_count": 0,
        "comparable_window_count": 0,
        "reason": reason,
        "alignment_series": [],
        "alignment_similarity": [],
        PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
        "limitations": [
            "Unavailable comparison is not evidence that routed audio was absent from the export."
        ],
    })
}

pub struct UnavailableAssociator;

impl ExportAssociator for UnavailableAssociator {
    fn associate(&self, _export_path: &Path, _routed_features: &[Value]) -> Value {
        unavailable_association("no export feature comparison engine is configured")
    }
}

/// The statistical forgery screen seam.
pub trait ForgeryAnalyzer: Send + Sync {
    fn analyze(&self, events: &[Value]) -> Value;
}

pub struct UnavailableForgeryAnalysis;

impl ForgeryAnalyzer for UnavailableForgeryAnalysis {
    /// IMPORTANT: no analyzer ran, so nothing is claimed in either direction. An
    /// empty flag list from a screen that never executed must not read as a clean
    /// result, which is why the status is explicit and the proof level is
    /// `unknown_unobserved` rather than the analyzed record's `inferred`.
    fn analyze(&self, _events: &[Value]) -> Value {
        json!({
            PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
            "status": "not_analyzed",
            "method": "none",
            "scope": "No statistical screen was run for this session. This is not a clean result; \
                      it records that the screen did not execute.",
            "analyzers": {},
        })
    }
}

pub struct ClaimRequest<'a> {
    pub export_path: &'a Path,
    pub export_hash: &'a str,
    pub stems: &'a [StemEvidence],
    pub ingredients: &'a [IngredientEvidence],
    /// Sample references read out of the saved project that the daemon never
    /// observed being imported. An implementation hashes them into `inputTo`
    /// ingredient nodes at `unknown_unobserved`, and records the ones it cannot
    /// resolve rather than dropping them.
    pub project_sample_refs: &'a [String],
    pub signed_asset_path: PathBuf,
    pub sidecar_path: PathBuf,
    pub manifest_dir: PathBuf,
}

/// The C2PA signing seam.
///
/// IMPORTANT: the export file itself is never rewritten. `export.sha256` is
/// committed before the claim is issued, so an in-place rewrite would make the
/// manifest describe a file that no longer exists; the signed copy goes to
/// `signed_asset_path`.
pub trait ClaimIssuer: Send + Sync {
    fn issue(&self, request: &ClaimRequest) -> Value;
}

pub struct UnavailableClaimIssuer;

impl ClaimIssuer for UnavailableClaimIssuer {
    fn issue(&self, _request: &ClaimRequest) -> Value {
        apw_core::unavailable_c2pa_claim("No C2PA engine is configured in this runtime.")
    }
}

#[derive(Debug, Clone)]
pub struct Seal {
    /// The `manifest_signature` record.
    pub record: Value,
    /// The cosignature hash the next manifest in this session entangles.
    pub entangled_hash: String,
}

/// The device-binding and local-integrity-seal seam.
///
/// A self-issued local seal proves that the sealed bytes have not changed when
/// re-checked with the same key. It is not hardware attestation and not identity.
pub trait LocalSealer: Send + Sync {
    fn bind_chain_root(&self, hash_chain_root: &str) -> Option<Value>;
    fn seal(
        &self,
        signing_input: &[u8],
        signed_content_hash: &str,
        previous_cosignature_hash: &str,
    ) -> Option<Seal>;
}

/// No sealer configured: the manifest carries the portable Ed25519 signature and
/// no local seal, which is exactly what the Python daemon writes when signing
/// raises.
pub struct UnsealedManifest;

impl LocalSealer for UnsealedManifest {
    fn bind_chain_root(&self, _hash_chain_root: &str) -> Option<Value> {
        None
    }

    fn seal(
        &self,
        _signing_input: &[u8],
        _signed_content_hash: &str,
        _previous_cosignature_hash: &str,
    ) -> Option<Seal> {
        None
    }
}

/// The RFC 3161 time-anchor seam. Absent, the manifest carries no `time_anchor`.
pub trait TimeAnchor: Send + Sync {
    fn anchor_record(&self, export_hash: &str) -> Value;
}
