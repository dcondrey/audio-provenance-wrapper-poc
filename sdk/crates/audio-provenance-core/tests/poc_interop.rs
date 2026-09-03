#![allow(clippy::unwrap_used, clippy::panic)]

use audio_provenance_core::signing::{PortableSignature, signer_id_for_public_key};
use audio_provenance_core::vocabulary::{AssociationStatus, CoverageStatus};
use audio_provenance_core::{
    AssociationClaim, CodedError, ObservationCoverage, ProofLevel, SignatureError, SigningKey,
    canonical_json, sha256_hex, unsigned_manifest_view, verify_manifest_signature, window_hash,
};
use serde_json::Value;

const MANIFESTS: [&str; 2] = ["poc_signed_manifest.json", "poc_signed_manifest_2.json"];

fn fixture(name: &str) -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read_to_string(format!("{dir}{name}")).unwrap()
}

fn manifest(name: &str) -> Value {
    serde_json::from_str(&fixture(name)).unwrap()
}

fn split(name: &str) -> (Value, PortableSignature) {
    let manifest = manifest(name);
    let signature: PortableSignature =
        serde_json::from_value(manifest.get("portable_signature").cloned().unwrap()).unwrap();
    (unsigned_manifest_view(&manifest), signature)
}

/// The load-bearing interop proof: real manifests written by the Python daemon, verified here with
/// no re-canonicalisation on the Python side.
#[test]
fn verifies_real_poc_manifest_signatures() {
    for name in MANIFESTS {
        let (unsigned, signature) = split(name);
        let proof = verify_manifest_signature(&unsigned, &signature, None)
            .unwrap_or_else(|err| panic!("{name}: {err} ({})", err.code()));
        assert_eq!(
            proof.signer_identity_proof_level(),
            ProofLevel::UnknownUnobserved
        );
        assert!(
            proof.signer_id_matches_declaration(&signature.signer_id),
            "{name}: recomputed signer_id disagrees with the manifest"
        );
        assert_eq!(
            sha256_hex(&canonical_json(&unsigned).unwrap()),
            signature.signed_content_hash,
            "{name}: canonical content hash diverges from the Python daemon"
        );
    }
}

/// Pins the ordering that makes the verifier honest: a manifest whose bytes were altered is
/// rejected on the content hash, before any Ed25519 work, exactly as `daemon/signing.py` does it.
#[test]
fn content_hash_is_checked_before_the_signature() {
    let (unsigned, signature) = split(MANIFESTS[0]);

    let mut tampered = unsigned.clone();
    if let Value::Object(map) = &mut tampered {
        map.insert("session_id".into(), Value::from("forged"));
    }
    let err = verify_manifest_signature(&tampered, &signature, None).unwrap_err();
    assert!(matches!(err, SignatureError::ContentHashMismatch));
    assert_eq!(err.code(), "signature_content_hash_mismatch");

    let mut lying_hash = signature.clone();
    lying_hash.signed_content_hash = sha256_hex(&canonical_json(&tampered).unwrap());
    let err = verify_manifest_signature(&tampered, &lying_hash, None).unwrap_err();
    assert!(
        matches!(err, SignatureError::InvalidSignature),
        "a rewritten signed_content_hash must fall through to the Ed25519 check, got {err:?}"
    );
}

#[test]
fn a_key_from_disk_must_be_exactly_thirty_two_raw_bytes() {
    for length in [0usize, 31, 33, 64] {
        let err = SigningKey::from_raw_bytes(&vec![7u8; length]).unwrap_err();
        assert_eq!(err.code(), "key_private_length_invalid");
    }
    let key = SigningKey::from_raw_bytes(&[7u8; 32]).unwrap();
    assert_eq!(
        key.signer_id(),
        signer_id_for_public_key(&key.public_key_bytes())
    );
    assert_eq!(key.signer_id().len(), 16);
}

#[test]
fn round_trips_a_signature_this_crate_produced() {
    let key = SigningKey::from_raw_bytes(&[42u8; 32]).unwrap();
    let manifest: Value =
        serde_json::from_str("{\"b\":[1,2.5],\"a\":\"\\u0000\\u001f ok\",\"z\":null,\"e\":1e-5}")
            .unwrap();
    let signature = key.sign_manifest(&manifest, "/dev/null").unwrap();

    let encoded = serde_json::to_value(&signature).unwrap();
    assert_eq!(encoded["canonicalization"], "apw-json-sort-v1");
    assert_eq!(encoded["trust_scope"], "self_generated_demo_key_integrity");
    assert_eq!(encoded["signer_identity_proof_level"], "unknown_unobserved");
    assert_eq!(encoded["apw:proof_level"], "directly_observed");

    let decoded: PortableSignature = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, signature);
    verify_manifest_signature(&manifest, &decoded, None).unwrap();
}

/// A wire signature that claims its signer identity is anything but `unknown_unobserved` must not
/// be constructible, whatever route it arrives by.
#[test]
fn a_signature_cannot_deserialize_into_an_overstated_identity() {
    let mut wire: Value = serde_json::from_value(
        manifest(MANIFESTS[0])
            .get("portable_signature")
            .cloned()
            .unwrap(),
    )
    .unwrap();

    for (field, forged) in [
        ("signer_identity_proof_level", "externally_verified"),
        ("signer_identity", "verified_creator"),
        ("trust_scope", "hardware_attested"),
    ] {
        let mut forged_wire = wire.clone();
        forged_wire[field] = Value::from(forged);
        assert!(
            serde_json::from_value::<PortableSignature>(forged_wire).is_err(),
            "{field}={forged} must be refused"
        );
    }

    wire["public_key_hex"] = Value::from("00");
    assert!(serde_json::from_value::<PortableSignature>(wire).is_err());
}

#[test]
fn window_hash_matches_the_poc_chained_hash() {
    let payload: Value = serde_json::from_str(&fixture("window_hash_cases.json")).unwrap();
    let cases = payload["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let prev = case["prev"].as_str().unwrap();
        let samples: Vec<f32> = case["samples"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        assert_eq!(
            window_hash(prev, &samples),
            case["sha256"].as_str().unwrap(),
            "chained hash diverges for prev={prev} over {} samples",
            samples.len()
        );
    }
    assert_eq!(window_hash("", &[0.25]), window_hash("genesis", &[0.25]));
}

/// The real manifest's counters must derive `complete_observed_path` from evidence alone, and a
/// manifest that declares more than its counters support must be refused.
#[test]
fn coverage_and_association_are_derived_not_declared() {
    let manifest = manifest(MANIFESTS[0]);

    let mut coverage_value = manifest.get("observation_coverage").cloned().unwrap();
    // The immutable interop fixture predates host-bypass telemetry. A current producer must add
    // this explicit zero before it may preserve the historical complete-coverage claim.
    coverage_value["counters"]["bypassed_buffers"] = Value::from(0u8);
    let coverage: ObservationCoverage = serde_json::from_value(coverage_value.clone()).unwrap();
    assert_eq!(coverage.status(), CoverageStatus::CompleteObservedPath);
    assert_eq!(coverage.proof_level(), ProofLevel::Inferred);

    let mut degraded = coverage_value.clone();
    degraded["counters"]["sequence_gaps"] = Value::from(1u8);
    let err = serde_json::from_value::<ObservationCoverage>(degraded.clone()).unwrap_err();
    assert!(
        err.to_string().contains("partial_observed_path"),
        "a session with a sequence gap must fall back to partial: {err}"
    );

    let mut promoted = coverage_value.clone();
    promoted["apw:proof_level"] = Value::from("directly_observed");
    assert!(serde_json::from_value::<ObservationCoverage>(promoted).is_err());

    let mut uncountable = coverage_value;
    uncountable
        .as_object_mut()
        .unwrap()
        .remove("counters")
        .unwrap();
    assert!(serde_json::from_value::<ObservationCoverage>(uncountable).is_err());

    let association_value = manifest.get("stem_export_association").cloned().unwrap();
    let association: AssociationClaim = serde_json::from_value(association_value.clone()).unwrap();
    assert_eq!(association.status(), AssociationStatus::InferredMatch);
    assert_eq!(association.proof_level(), ProofLevel::Inferred);

    let mut overstated = association_value;
    overstated["apw:proof_level"] = Value::from("directly_observed");
    let err = serde_json::from_value::<AssociationClaim>(overstated).unwrap_err();
    assert!(err.to_string().contains("must remain inferred"), "{err}");

    assert_eq!(
        AssociationClaim::established().proof_level(),
        ProofLevel::Inferred
    );
    assert_eq!(
        AssociationClaim::not_established().proof_level(),
        ProofLevel::UnknownUnobserved
    );
    assert_eq!(
        ObservationCoverage::unknown().status(),
        CoverageStatus::UnknownCoverage
    );
}

#[test]
fn a_legacy_complete_claim_without_bypass_telemetry_is_refused() {
    let manifest = manifest(MANIFESTS[0]);
    let coverage_value = manifest.get("observation_coverage").cloned().unwrap();
    let error = serde_json::from_value::<ObservationCoverage>(coverage_value).unwrap_err();
    assert!(
        error.to_string().contains("partial_observed_path"),
        "{error}"
    );
}
