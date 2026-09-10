//! Parity with the Python manifest builder, schema validator and portable
//! signer. Expected values come from `tests/generate_parity_fixtures.py`, which
//! drives `daemon.manifest_builder.builder`, `daemon.schema` and
//! `daemon.signing` directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;

use apw_core::{
    canonical_json_utf8, pretty_json_bytes, validate_manifest_invariants,
    verify_portable_signature, Ed25519Signer, ExportEvidence, IngredientEvidence, ManifestBuilder,
    PortableSignature, ProofLevel, SignatureRejection, StemEvidence,
    PORTABLE_SIGNATURE_VALID_MESSAGE,
};
use serde_json::{json, Number, Value};

const CREATED_AT: &str = "2025-08-31T00:26:40.500000Z";

fn load(name: &str) -> Value {
    let raw = match name {
        "builder_cases.json" => include_str!("fixtures/builder_cases.json"),
        "schema_cases.json" => include_str!("fixtures/schema_cases.json"),
        "signature_case.json" => include_str!("fixtures/signature_case.json"),
        "signing_input_case.json" => include_str!("fixtures/signing_input_case.json"),
        other => panic!("unknown fixture {other}"),
    };
    serde_json::from_str(raw).expect("fixture is valid JSON")
}

fn cases_by_name(fixture: &Value) -> HashMap<String, Value> {
    fixture
        .as_array()
        .expect("fixture root is an array")
        .iter()
        .map(|case| {
            let name = case
                .get("name")
                .and_then(Value::as_str)
                .expect("case has a name");
            (name.to_owned(), case.clone())
        })
        .collect()
}

fn empty_builder() -> ManifestBuilder {
    ManifestBuilder::new("", CREATED_AT.to_owned())
}

fn stem_only_builder() -> ManifestBuilder {
    let mut builder = ManifestBuilder::new("s2", CREATED_AT.to_owned());
    builder.add_stem(StemEvidence {
        stem_id: "stem-only".to_owned(),
        hash_chain_root: "e".repeat(64),
        hash_chain_genesis: "genesis".to_owned(),
        hash_chain_length: 3,
        first_observed_ms: 0,
        last_observed_ms: 1,
        first_received_at: None,
        last_received_at: None,
        sample_rate_hz: 44100,
        channel_count: 2,
        source_category: "unknown".to_owned(),
        source_category_proof_level: ProofLevel::UnknownUnobserved,
        plugin_instance_ids: Vec::new(),
        proof_level: ProofLevel::DirectlyObserved,
    });
    builder
}

fn full_builder() -> ManifestBuilder {
    let mut builder = ManifestBuilder::new("sess-café-01", CREATED_AT.to_owned());
    builder.add_stem(StemEvidence {
        stem_id: "stem-1".to_owned(),
        hash_chain_root: "a".repeat(64),
        hash_chain_genesis: "genesis".to_owned(),
        hash_chain_length: 12,
        first_observed_ms: 1000,
        last_observed_ms: 5000,
        first_received_at: Some(CREATED_AT.to_owned()),
        last_received_at: None,
        sample_rate_hz: 44100,
        channel_count: 2,
        source_category: "synthesised".to_owned(),
        source_category_proof_level: ProofLevel::UserDeclared,
        plugin_instance_ids: vec!["plug-1".to_owned(), "plug-2".to_owned()],
        proof_level: ProofLevel::DirectlyObserved,
    });
    builder.add_stem(StemEvidence {
        stem_id: "stem-é-2".to_owned(),
        hash_chain_root: "b".repeat(64),
        hash_chain_genesis: "genesis".to_owned(),
        hash_chain_length: 7,
        first_observed_ms: -1,
        last_observed_ms: 0,
        first_received_at: None,
        last_received_at: None,
        sample_rate_hz: 48000,
        channel_count: 1,
        source_category: "recorded".to_owned(),
        source_category_proof_level: ProofLevel::UnknownUnobserved,
        plugin_instance_ids: Vec::new(),
        proof_level: ProofLevel::Inferred,
    });
    builder.set_export(ExportEvidence {
        file_path: "/tmp/Bounce – final.wav".to_owned(),
        file_name: "Bounce – final.wav".to_owned(),
        sha256: "c".repeat(64),
        format: "wav".to_owned(),
        file_size_bytes: 1_234_567,
        duration_seconds: Some(12.345678901234567),
        sample_rate_hz: Number::from_f64(44100.0),
        channel_count: Some(2),
        exported_at: "2025-08-31T00:26:40Z".to_owned(),
        export_version: 3,
    });
    builder.add_ingredient(IngredientEvidence {
        file_name: "kick.wav".to_owned(),
        sha256: "d".repeat(64),
        proof_level: ProofLevel::Inferred,
        correlation_confidence: Some(0.9375),
        audio_fingerprint: Some(json!({"rms": 0.0001220703125, "zcr": null})),
    });
    builder.add_composite_edit(
        json!({"edit_type": "clip_paste", "timestamp_ms": 42, "confidence": 0.5}),
    );
    builder.add_composite_edit(
        json!({"edit_type": "mystery", "timestamp_ms": null, "confidence": null}),
    );
    builder.set_hardware_binding(json!({"provider": "software", "apw:proof_level": "inferred"}));
    builder.add_time_anchor(json!({"source": "local_clock", "apw:proof_level": "inferred"}));
    builder.set_forgery_report(json!({"suspicion_score": 0.0, "flags": []}));
    builder.set_coverage(json!({
        "status": "complete_observed_path",
        "basis": "All hashed windows were received.",
        "counters": {"windows_hashed": 19, "buffer_hash_events_received": 19},
        "apw:proof_level": "directly_observed",
    }));
    builder.set_audio_association(json!({
        "status": "inferred_match",
        "method": "gain_normalised_offset_search",
        "confidence": 0.8125,
        "matched_coverage": 0.75,
        "apw:proof_level": "inferred",
    }));
    builder.set_session_diagnostics(json!({"udp_sends_failed": 0}));
    builder.set_host_environment(json!({
        "status": "observed",
        "host_recognised": true,
        "host_name": "Ableton Live",
        "host_executable_name": "Live",
        "wrapper_format": "VST3",
        "basis": "The plug-in wrapper named the host application that loaded it.",
        "scope": "host scope",
        "apw:proof_level": "directly_observed",
    }));
    builder.set_c2pa_claim(json!({
        "status": "embedded",
        "validation": {"state": "verified", "trust_anchor_scope": "self_issued_local_root_only"},
        "hard_binding": {"algorithm": "sha256"},
        "source_sha256_matches_export": true,
        "signer": {"signer_identity": "not_established", "apw:proof_level": "directly_observed"},
        "ingredients": [],
        "apw:proof_level": "directly_observed",
    }));
    builder
}

#[test]
fn builder_reproduces_the_python_manifest_byte_for_byte() {
    let fixture = load("builder_cases.json");
    let cases = cases_by_name(&fixture);
    let builders: [(&str, ManifestBuilder); 3] = [
        ("empty", empty_builder()),
        ("full", full_builder()),
        ("stem_only", stem_only_builder()),
    ];
    for (name, builder) in builders {
        let case = cases.get(name).unwrap_or_else(|| panic!("missing case {name}"));
        let expected_manifest = case.get("manifest").expect("case has a manifest");
        let expected_pretty = case
            .get("pretty")
            .and_then(Value::as_str)
            .expect("case has pretty output");

        let manifest = builder.build().expect("builder produces an object");

        // Insertion order and every value: the pretty rendering is order-bearing.
        assert_eq!(
            String::from_utf8_lossy(&manifest.to_pretty_bytes().unwrap()),
            expected_pretty,
            "builder case {name} diverged"
        );
        assert_eq!(
            canonical_json_utf8(manifest.as_value()).unwrap(),
            canonical_json_utf8(expected_manifest).unwrap(),
            "builder case {name} canonical bytes diverged"
        );
        assert_eq!(
            String::from_utf8_lossy(&pretty_json_bytes(expected_manifest).unwrap()),
            expected_pretty,
        );
    }
}

#[test]
fn schema_violations_match_the_python_validator_in_order_and_wording() {
    let fixture = load("schema_cases.json");
    for case in fixture.as_array().expect("fixture root is an array") {
        let name = case
            .get("name")
            .and_then(Value::as_str)
            .expect("case has a name");
        let manifest = case.get("manifest").expect("case has a manifest");
        let expected: Vec<String> = case
            .get("errors")
            .and_then(Value::as_array)
            .expect("case has errors")
            .iter()
            .map(|error| error.as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            validate_manifest_invariants(manifest),
            expected,
            "schema case {name} diverged"
        );
    }
}

#[test]
fn a_python_produced_signature_verifies_and_a_tampered_manifest_does_not() {
    let fixture = load("signature_case.json");
    let unsigned = fixture
        .get("unsigned_manifest")
        .expect("fixture has an unsigned manifest")
        .clone();
    let signature: PortableSignature =
        serde_json::from_value(fixture.get("signature").expect("fixture has a signature").clone())
            .expect("the Python signature deserializes into PortableSignature");

    assert_eq!(
        verify_portable_signature(&unsigned, &signature, None),
        Ok(PORTABLE_SIGNATURE_VALID_MESSAGE)
    );

    let mut tampered = unsigned.clone();
    if let Value::Object(map) = &mut tampered {
        map.insert("session_id".to_owned(), json!("someone-elses-session"));
    }
    assert_eq!(
        verify_portable_signature(&tampered, &signature, None),
        Err(SignatureRejection::ContentHashMismatch)
    );

    let mut forged = signature.clone();
    forged.signature_hex = "00".repeat(64);
    assert_eq!(
        verify_portable_signature(&unsigned, &forged, None),
        Err(SignatureRejection::SignatureInvalid)
    );

    let mut short_key = signature.clone();
    short_key.public_key_hex = "aabb".to_owned();
    assert_eq!(
        verify_portable_signature(&unsigned, &short_key, None),
        Err(SignatureRejection::PublicKeyWrongLength)
    );

    let mut bad_hex = signature.clone();
    bad_hex.public_key_hex = "zz".repeat(32);
    assert_eq!(
        verify_portable_signature(&unsigned, &bad_hex, None),
        Err(SignatureRejection::MalformedPublicKey)
    );
}

#[test]
fn rust_signing_reproduces_the_python_signature_bytes() {
    let fixture = load("signature_case.json");
    let seed_hex = fixture
        .get("seed_hex")
        .and_then(Value::as_str)
        .expect("fixture has a seed");
    let seed: Vec<u8> = (0..seed_hex.len() / 2)
        .map(|index| {
            u8::from_str_radix(&seed_hex[index * 2..index * 2 + 2], 16).expect("seed is hex")
        })
        .collect();

    let directory = tempfile::tempdir().expect("temp dir");
    let private_path = directory.path().join("demo_ed25519_private.key");
    let public_path = directory.path().join("demo_ed25519_public.key");
    std::fs::write(&private_path, &seed).expect("seed written");

    let signer = Ed25519Signer::load_or_create(&private_path, &public_path).expect("signer loads");
    let unsigned = fixture
        .get("unsigned_manifest")
        .expect("fixture has an unsigned manifest")
        .clone();
    let produced = signer.sign_manifest(&unsigned).expect("signing succeeds");

    let expected: PortableSignature =
        serde_json::from_value(fixture.get("signature").expect("fixture has a signature").clone())
            .expect("the Python signature deserializes");

    // Every field except `public_key_file`, which carries the generator's
    // temporary directory rather than a stable path.
    assert_eq!(produced.algorithm, expected.algorithm);
    assert_eq!(produced.canonicalization, expected.canonicalization);
    assert_eq!(produced.public_key_hex, expected.public_key_hex);
    assert_eq!(produced.signer_id, expected.signer_id);
    assert_eq!(produced.signature_hex, expected.signature_hex);
    assert_eq!(produced.signed_content_hash, expected.signed_content_hash);
    assert_eq!(produced.trust_scope, expected.trust_scope);
    assert_eq!(produced.signer_identity, expected.signer_identity);
    assert_eq!(
        produced.signer_identity_proof_level,
        expected.signer_identity_proof_level
    );
    assert_eq!(produced.proof_level, expected.proof_level);
    assert_eq!(produced.notes, expected.notes);
    assert_eq!(
        std::fs::read(&public_path).expect("public key written"),
        signer.public_key_bytes()
    );
}

/// The two signing inputs over a manifest that already carries both signature
/// keys. IMPORTANT: `local_signing_input` covers `portable_signature`, so the
/// portable signature must be attached before the local one is computed, and
/// the local input is ascii-escaped while the portable one is raw UTF-8. A
/// wrong exclusion set here is silent and breaks every signature.
#[test]
fn signing_inputs_match_the_python_exclusion_sets() {
    let fixture = load("signing_input_case.json");
    let manifest = apw_core::Manifest::from_value(
        fixture.get("manifest").expect("fixture has a manifest").clone(),
    )
    .expect("manifest is an object");

    let expected_portable = fixture
        .get("portable_signing_input")
        .and_then(Value::as_str)
        .expect("fixture has the portable input");
    let expected_local = fixture
        .get("local_signing_input")
        .and_then(Value::as_str)
        .expect("fixture has the local input");

    assert_eq!(
        String::from_utf8_lossy(&manifest.portable_signing_input().unwrap()),
        expected_portable
    );
    assert_eq!(
        String::from_utf8_lossy(&manifest.local_signing_input().unwrap()),
        expected_local
    );
    assert!(
        expected_portable.contains("sess-café-01"),
        "the portable input must carry raw UTF-8"
    );
    assert!(
        expected_local.contains("sess-caf\\u00e9-01"),
        "the local input must be ascii-escaped"
    );
    assert!(
        !expected_portable.contains("portable_signature")
            && !expected_portable.contains("manifest_signature"),
        "the portable input excludes both signature keys"
    );
    assert!(
        expected_local.contains("portable_signature")
            && !expected_local.contains("manifest_signature"),
        "the local input keeps portable_signature and drops manifest_signature"
    );

    // The still-attached portable signature must verify against the input the
    // exclusion set produces.
    let signature: PortableSignature = serde_json::from_value(
        manifest
            .get("portable_signature")
            .expect("manifest carries a portable signature")
            .clone(),
    )
    .expect("signature deserializes");
    let unsigned = apw_core::without_top_level_keys(
        manifest.as_value(),
        &apw_core::PORTABLE_SIGNATURE_EXCLUDED_KEYS,
    );
    assert_eq!(
        verify_portable_signature(&unsigned, &signature, None),
        Ok(PORTABLE_SIGNATURE_VALID_MESSAGE)
    );
}

/// `daemon/bundle.py:128` renders the evidence index with `indent=2` AND
/// `sort_keys=True`, and those bytes are hashed into the published bundle.
#[test]
fn bundle_index_rendering_sorts_keys() {
    let fixture = load("signing_input_case.json");
    let index = fixture.get("bundle_index").expect("fixture has an index");
    let expected = fixture
        .get("bundle_index_pretty_sorted")
        .and_then(Value::as_str)
        .expect("fixture has the rendering");
    assert_eq!(
        String::from_utf8_lossy(&apw_core::pretty_json_sorted_bytes(index).unwrap()),
        expected
    );
    assert_ne!(
        apw_core::pretty_json_sorted_bytes(index).unwrap(),
        pretty_json_bytes(index).unwrap(),
        "the sorted and insertion-ordered renderings must not be conflated"
    );
}
