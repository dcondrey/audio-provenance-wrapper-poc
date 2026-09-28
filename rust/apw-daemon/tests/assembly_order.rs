//! The assembly sequence is a security property: everything a signature must
//! cover is attached before that signature is computed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;
use std::sync::Mutex;

use apw_core::{
    canonical_json_ascii, sha256_hex, verify_portable_signature, without_top_level_keys,
    Ed25519Signer, PortableSignature, ProofLevel, LOCAL_SIGNATURE_EXCLUDED_KEYS,
    PORTABLE_SIGNATURE_EXCLUDED_KEYS,
};
use apw_daemon::{
    derive_coverage, generate_manifest, AssemblyContext, AssemblyInputs, CoverageInputs,
    EvidenceReceiver, LocalSealer, ManifestServices, Seal, SessionState, UnavailableAssociator,
    UnavailableAudioProbe, UnavailableClaimIssuer, UnavailableForgeryAnalysis,
};
use serde_json::{json, Value};

/// Records exactly what the local seal was asked to sign, so the test can assert
/// what the seal actually commits to instead of trusting the call order.
#[derive(Default)]
struct RecordingSealer {
    signed: Mutex<Option<Vec<u8>>>,
    bound_root: Mutex<Option<String>>,
}

impl LocalSealer for RecordingSealer {
    fn bind_chain_root(&self, hash_chain_root: &str) -> Option<Value> {
        if let Ok(mut bound) = self.bound_root.lock() {
            *bound = Some(hash_chain_root.to_owned());
        }
        Some(json!({
            "device_id": "test-device",
            "hash_chain_root": hash_chain_root,
            "hardware_attested": false,
            apw_core::PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
        }))
    }

    fn seal(
        &self,
        signing_input: &[u8],
        signed_content_hash: &str,
        previous_cosignature_hash: &str,
    ) -> Option<Seal> {
        if let Ok(mut signed) = self.signed.lock() {
            *signed = Some(signing_input.to_vec());
        }
        Some(Seal {
            record: json!({
                "algorithm": "HMAC-SHA256",
                "signed_content_hash": signed_content_hash,
                "trust_scope": apw_core::TRUST_SCOPE_LOCAL_SOFTWARE,
                "hardware_attested": false,
                "hardware_cosignature": {"previous": previous_cosignature_hash},
                apw_core::PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
            }),
            entangled_hash: format!("entangled-{signed_content_hash}"),
        })
    }
}

fn routed_window(sequence: i64, prev: &str, window: &str) -> Value {
    json!({
        "event_type": "buffer_hash",
        "proof_level": "directly_observed",
        "plugin_instance_id": "instance-a",
        "plugin_capture_session_id": "plugin-session",
        "event_sequence": sequence,
        "source_timestamp_ms": 1_000 + sequence,
        "received_at": format!("2026-08-31T00:00:{sequence:02}Z"),
        "window_hash": window,
        "prev_hash": prev,
        "rms_level": 0.25,
        "zero_crossing_rate": 0.1,
        "sample_rate_hz": 48_000,
        "channel_count": 2,
        "window_size_samples": 4_096,
        "telemetry": {"windows_hashed": sequence},
    })
}

#[test]
fn both_signatures_cover_everything_assembled_before_them() {
    let workspace = tempfile::tempdir().expect("tempdir");
    let evidence_dir = workspace.path().join("evidence");
    let manifest_dir = workspace.path().join("manifests");
    let export_dir = workspace.path().join("exports");
    std::fs::create_dir_all(&evidence_dir).expect("evidence dir");
    std::fs::create_dir_all(&manifest_dir).expect("manifest dir");
    std::fs::create_dir_all(&export_dir).expect("export dir");
    std::fs::write(evidence_dir.join("plugin_events.jsonl"), b"{\"a\":1}\n").expect("evidence");
    let export_path = export_dir.join("take.wav");
    std::fs::write(&export_path, b"RIFF....WAVEfmt ").expect("export");

    let mut session = SessionState::new();
    session.record_plugin_event(&routed_window(1, "genesis", "w1"), "audio_buffer");
    session.record_plugin_event(&routed_window(2, "w1", "w2"), "audio_buffer");
    session.append_event(json!({
        "event_type": "sample_file_observed",
        "proof_level": "directly_observed",
        "file_name": "kick.wav",
        "sha256": "a".repeat(64),
        "audio_fingerprint": {"rms": 0.2, "zero_crossing_rate": 0.05},
    }));
    let snapshot = session.snapshot();

    let receiver = EvidenceReceiver::bind("127.0.0.1", 0, &evidence_dir.join("plugin_events.jsonl"), None, None)
        .expect("bind");
    let receiver_diagnostics = receiver.diagnostics();
    let coverage = derive_coverage(&CoverageInputs {
        telemetry: &snapshot.telemetry,
        receiver: &receiver_diagnostics,
        plugin_instance_count: 1,
        chain_length: snapshot.chain_length,
        feature_window_drops: snapshot.feature_window_drops,
        telemetry_regressions: snapshot.telemetry_regressions,
    });

    let signer = Ed25519Signer::load_or_create(
        &workspace.path().join("signing_key.bin"),
        &workspace.path().join("signing_key.pub"),
    )
    .expect("signer");
    let sealer = RecordingSealer::default();

    let context = AssemblyContext {
        session_id: "capture-test",
        stem_id: "stem-1",
        source_category: "midi_vst_synth",
        source_category_proof_level: ProofLevel::UserDeclared,
        evidence_dir: &evidence_dir,
        manifest_dir: &manifest_dir,
        session_started_at: "2026-08-31T00:00:00Z",
        generate_html_report: true,
        previous_cosignature_hash: "genesis",
    };
    let services = ManifestServices {
        audio: &UnavailableAudioProbe,
        associator: &UnavailableAssociator,
        forgery: &UnavailableForgeryAnalysis,
        claims: &UnavailableClaimIssuer,
        sealer: &sealer,
        portable_signer: Some(&signer),
        time_anchor: None,
    };
    let inputs = AssemblyInputs {
        snapshot: &snapshot,
        coverage,
        session_diagnostics: json!({"receiver": receiver_diagnostics.to_json()}),
        receipt_summary: receiver.receipt_summary(),
        session_facts: None,
        project_sample_refs: Vec::new(),
    };

    let generated = generate_manifest(&context, &services, &inputs, &export_path, 1)
        .expect("manifest assembly");
    let manifest = &generated.manifest;
    let document = manifest.as_object();
    let keys: Vec<&str> = document.keys().map(String::as_str).collect();
    let position = |key: &str| keys.iter().position(|name| *name == key);

    let hardware = position("hardware_binding").expect("hardware_binding");
    let portable = position("portable_signature").expect("portable_signature");
    let local = position("manifest_signature").expect("manifest_signature");
    assert!(
        hardware < portable && portable < local,
        "binding, then portable signature, then local seal: {keys:?}"
    );

    // The portable signature verifies over the document minus both signatures,
    // which is only true if every earlier step was already attached.
    let signature: PortableSignature = serde_json::from_value(
        document
            .get("portable_signature")
            .cloned()
            .expect("portable signature record"),
    )
    .expect("portable signature shape");
    let unsigned = without_top_level_keys(manifest.as_value(), &PORTABLE_SIGNATURE_EXCLUDED_KEYS);
    assert_eq!(
        verify_portable_signature(&unsigned, &signature, None),
        Ok(apw_core::PORTABLE_SIGNATURE_VALID_MESSAGE)
    );
    assert_eq!(signature.trust_scope, apw_core::TRUST_SCOPE_SELF_GENERATED);
    assert_eq!(signature.signer_identity, "not_established");
    assert_eq!(
        signature.signer_identity_proof_level,
        ProofLevel::UnknownUnobserved,
        "a self-issued key proves possession, never identity"
    );

    // The local seal's input is the ASCII canonicalisation of the document minus
    // manifest_signature, so it must contain the portable signature and the
    // chain-root binding.
    let sealed_input = sealer
        .signed
        .lock()
        .ok()
        .and_then(|signed| signed.clone())
        .expect("seal input recorded");
    let expected_input = canonical_json_ascii(&without_top_level_keys(
        manifest.as_value(),
        &LOCAL_SIGNATURE_EXCLUDED_KEYS,
    ))
    .expect("local signing input");
    assert_eq!(sealed_input, expected_input);
    let sealed_text = String::from_utf8(sealed_input.clone()).expect("utf-8");
    assert!(sealed_text.contains("portable_signature"));
    assert!(sealed_text.contains("hardware_binding"));
    assert!(!sealed_text.contains("manifest_signature"));
    assert_eq!(
        document
            .get("manifest_signature")
            .and_then(|record| record.get("signed_content_hash"))
            .and_then(Value::as_str),
        Some(sha256_hex(&sealed_input).as_str())
    );
    assert_eq!(
        generated.cosignature_hash.as_deref(),
        Some(format!("entangled-{}", sha256_hex(&sealed_input)).as_str())
    );
    assert_eq!(
        sealer.bound_root.lock().ok().and_then(|root| root.clone()),
        Some("w2".to_owned()),
        "the binding commits to the last observed window hash"
    );

    // The claim-bearing content the signatures now cover.
    assert_eq!(
        document.get("evidence_binding").and_then(|b| b.get("chain_length")),
        Some(&json!(2))
    );
    assert_eq!(
        document
            .get("capture_session")
            .and_then(|session| session.get("state_at_manifest")),
        Some(&json!("active"))
    );
    assert_eq!(
        document
            .get("claim_summary")
            .and_then(|summary| summary.get(2))
            .and_then(|entry| entry.get("claim")),
        Some(&json!("daemon_receipt_acknowledgement"))
    );
    let unobserved = document
        .get("apw:unobserved")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for layer in ["transport", "midi", "session", "project_differ", "input_capture", "screen_observer"] {
        assert!(
            unobserved.contains(&json!(format!("layer_{layer}_not_active"))),
            "an inactive layer must be declared unobserved: {layer}"
        );
    }
    assert!(!unobserved.contains(&json!("layer_audio_buffer_not_active")));
    assert_eq!(
        document.get("c2pa_claim").and_then(|claim| claim.get("status")),
        Some(&json!("unavailable")),
        "no C2PA engine was configured, and the manifest says so rather than implying a claim"
    );

    assert_eq!(
        document
            .get("observed_stems")
            .and_then(|stems| stems.get(0))
            .and_then(|stem| stem.get("first_received_at")),
        Some(&json!("2026-08-31T00:00:01Z")),
        "the stem carries the daemon's own receipt times, not the plug-in's clock"
    );

    // Everything written to disk is the signed document, byte for byte.
    let written = std::fs::read(&generated.manifest_path).expect("manifest file");
    assert_eq!(written, manifest.to_pretty_bytes().expect("pretty bytes"));
    assert!(Path::new(&generated.handoff_path).is_file());

    // The assembly order against a manifest the Python pipeline actually wrote.
    #[derive(serde::Deserialize)]
    struct KeyOrder {
        keys: Vec<String>,
    }
    let oracle: KeyOrder = serde_json::from_str(include_str!("fixtures/manifest_key_order.json"))
        .expect("manifest key order fixture");
    let shared_oracle: Vec<&String> = oracle
        .keys
        .iter()
        .filter(|key| document.contains_key(key.as_str()))
        .collect();
    let shared_document: Vec<&String> = document
        .keys()
        .filter(|key| oracle.keys.contains(key))
        .collect();
    assert_eq!(
        shared_document, shared_oracle,
        "top-level key order must match the Python pipeline's own manifest"
    );
    assert!(
        shared_oracle.len() >= 20,
        "the comparison must cover the whole document, not a handful of keys"
    );
}
