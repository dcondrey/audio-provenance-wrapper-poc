#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use audio_provenance_audio::{AudioBuffer, BitDepth, wav};
use audio_provenance_core::{ProofLevel, SigningKey, VerificationStatus, sha256_hex};
use audio_provenance_manifest::{ManifestSchema, UnverifiedManifest};
use audio_provenance_registry::LocalRegistryBackend;
use audio_provenance_sdk::{
    CaptureAdapterOptions, SidecarOutput, adapt_capture_export, write_capture_adapter_receipt,
};
use serde_json::Value;

const POC_MANIFEST: &str =
    include_str!("../../audio-provenance-manifest/tests/fixtures/poc_signed_manifest.json");

fn write_export(path: &Path) -> String {
    let rate = 44_100u32;
    let samples: Vec<f32> = (0..rate as usize * 2)
        .map(|index| {
            let t = index as f32 / rate as f32;
            0.35 * (t * 440.0 * std::f32::consts::TAU).sin()
                + 0.1 * (t * 1_730.0 * std::f32::consts::TAU).sin()
        })
        .collect();
    let audio = AudioBuffer::from_channels(rate, &[samples]).unwrap();
    let bytes = wav::encode(&audio, BitDepth::Int24).unwrap();
    let digest = sha256_hex(&bytes);
    std::fs::write(path, bytes).unwrap();
    digest
}

fn write_current_capture_inputs(
    root: &Path,
    export_sha256: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let mut value: Value = serde_json::from_str(POC_MANIFEST).unwrap();
    value["export"]["sha256"] = Value::String(export_sha256.to_string());
    value["downstream_registration_handoff"]["export_hard_hash"]["value"] =
        Value::String(export_sha256.to_string());
    value["observation_coverage"]["counters"]["bypassed_buffers"] = Value::from(0u8);
    value["downstream_registration_handoff"]["coverage"]["counters"]["bypassed_buffers"] =
        Value::from(0u8);
    value.as_object_mut().unwrap().remove("portable_signature");
    value.as_object_mut().unwrap().remove("manifest_signature");
    let capture_key = SigningKey::from_raw_bytes(&[3u8; 32]).unwrap();
    let signature = capture_key.sign_manifest(&value, "capture.pub").unwrap();
    value["portable_signature"] = serde_json::to_value(signature).unwrap();

    let manifest = root.join("capture.json");
    let handoff = root.join("handoff.json");
    std::fs::write(&manifest, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    std::fs::write(
        &handoff,
        serde_json::to_vec_pretty(&value["downstream_registration_handoff"]).unwrap(),
    )
    .unwrap();
    (manifest, handoff)
}

#[test]
fn capture_handoff_round_trips_idempotently_without_proof_promotion() {
    let directory = tempfile::tempdir().unwrap();
    let export = directory.path().join("export.wav");
    let export_sha256 = write_export(&export);
    let (capture_manifest, handoff) =
        write_current_capture_inputs(directory.path(), &export_sha256);
    let bundle = directory.path().join("evidence.zip");
    std::fs::write(&bundle, b"bounded synthetic evidence bundle").unwrap();
    let bundle_sha256 = sha256_hex(b"bounded synthetic evidence bundle");
    let sidecar = directory.path().join("export.aprv.json");
    let registry_path = directory.path().join("registry");
    let receipt = directory.path().join("adapter-receipt.json");

    let run = || {
        adapt_capture_export(
            &export,
            &capture_manifest,
            &handoff,
            &bundle,
            CaptureAdapterOptions::new(SigningKey::from_raw_bytes(&[9u8; 32]).unwrap())
                .public_key_file("fixture-development.key")
                .sidecar(SidecarOutput::Explicit(sidecar.clone()))
                .expected_evidence_bundle_sha256(bundle_sha256.clone())
                .unwrap()
                .registry(LocalRegistryBackend::init("temporary", &registry_path).unwrap()),
        )
        .unwrap()
    };

    let first = run();
    write_capture_adapter_receipt(&receipt, &first).unwrap();
    let second = run();

    assert_eq!(first.development_only, Some(true));
    assert_eq!(first.identity, Some("not_established"));
    assert_eq!(first.evidence_bundle_sha256, bundle_sha256);
    assert_eq!(first.sign.record_id, second.sign.record_id);
    assert_eq!(first.sign.manifest_bytes, second.sign.manifest_bytes);
    assert_eq!(
        first.publish.as_ref().unwrap().record_id,
        first.sign.record_id
    );
    assert_eq!(first.verification.status, VerificationStatus::Untrusted);
    assert_eq!(first.verification.r#match, 1.0);
    assert!(receipt.is_file());

    let admitted = UnverifiedManifest::parse(&first.sign.manifest_bytes)
        .unwrap()
        .admit(ManifestSchema::AudioProvenanceV1)
        .unwrap();
    assert_eq!(
        admitted.observation_coverage().unwrap().proof_level(),
        ProofLevel::Inferred
    );
    assert_eq!(
        admitted.association().unwrap().proof_level(),
        ProofLevel::Inferred
    );
    assert!(
        admitted.claims().iter().any(|claim| {
            claim.claim == "development_only"
                && claim.value == Value::Bool(true)
                && claim.proof_level == ProofLevel::DirectlyObserved
        }),
        "development-only metadata was not signed into the record"
    );
    assert_eq!(
        admitted
            .claims()
            .iter()
            .filter(|claim| claim.claim.starts_with("capture_handoff:"))
            .count(),
        first.proof_objects_preserved
    );
    assert!(
        admitted.claims().iter().any(|claim| {
            claim.claim == "full_ableton_provenance"
                && claim.proof_level == ProofLevel::UnknownUnobserved
        }),
        "an unknown proof level from the capture manifest was dropped or promoted"
    );
}

#[test]
fn a_handoff_not_covered_by_the_capture_signature_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let export = directory.path().join("export.wav");
    let export_sha256 = write_export(&export);
    let (capture_manifest, handoff) =
        write_current_capture_inputs(directory.path(), &export_sha256);
    let bundle = directory.path().join("evidence.zip");
    std::fs::write(&bundle, b"evidence").unwrap();

    let mut changed: Value = serde_json::from_slice(&std::fs::read(&handoff).unwrap()).unwrap();
    changed["capture_session_id"] = Value::String("somebody-else".to_string());
    std::fs::write(&handoff, serde_json::to_vec(&changed).unwrap()).unwrap();

    let error = adapt_capture_export(
        &export,
        &capture_manifest,
        &handoff,
        &bundle,
        CaptureAdapterOptions::new(SigningKey::from_raw_bytes(&[9u8; 32]).unwrap()).sidecar(
            SidecarOutput::Explicit(directory.path().join("record.json")),
        ),
    )
    .unwrap_err();
    assert!(error.to_string().contains("differs from the value covered"));
}

#[test]
fn production_custody_omits_development_and_identity_overrides() {
    let directory = tempfile::tempdir().unwrap();
    let export = directory.path().join("export.wav");
    let export_sha256 = write_export(&export);
    let (capture_manifest, handoff) =
        write_current_capture_inputs(directory.path(), &export_sha256);
    let bundle = directory.path().join("evidence.zip");
    std::fs::write(&bundle, b"production-mode fixture").unwrap();

    // A deterministic local signer stands in for the remote transport here; transport and returned
    // signature verification have their own wiremock suite. This test owns adapter metadata only.
    let result = adapt_capture_export(
        &export,
        &capture_manifest,
        &handoff,
        &bundle,
        CaptureAdapterOptions::production(SigningKey::from_raw_bytes(&[11; 32]).unwrap()).sidecar(
            SidecarOutput::Explicit(directory.path().join("production.json")),
        ),
    )
    .unwrap();
    assert_eq!(result.development_only, None);
    assert_eq!(result.identity, None);
    let admitted = UnverifiedManifest::parse(&result.sign.manifest_bytes)
        .unwrap()
        .admit(ManifestSchema::AudioProvenanceV1)
        .unwrap();
    assert!(
        admitted
            .claims()
            .iter()
            .all(|claim| claim.claim != "development_only")
    );
}
