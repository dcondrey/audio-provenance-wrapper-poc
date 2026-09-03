#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! A GenoMark payload may substitute for a hard binding a record never declared ONLY when it names
//! that record's own locator.
//!
//! Namespace 0's profile key is the published literal `b"audio-provenance/genomark/public/v1"`, so anyone
//! can mint a CRC-valid payload carrying any 48-bit locator, and locators are public registry keys.
//! Without the locator term, any record with no hard binding would be verified by any marked audio
//! at all. Both halves are driven end to end through `verify`, because the reachability of the arm
//! is the whole claim: the first test is what proves the surface was real, the second is what proves
//! it is closed.
//!
//! The fixture is an `audio-provenance-manifest-v0` document with no `export` block. That is a shape
//! the POC's own producer emits: `daemon/manifest_builder/builder.py` writes `export` only under
//! `if self.export is not None`, `daemon/schema.py` reads it with `data.get("export")`, and
//! `docs/manifest.schema.json` does not list it among the seventeen required fields.

use audio_provenance_audio::{AudioBuffer, BitDepth, wav};
use audio_provenance_core::{
    GENOMARK_PAYLOAD_VERSION, LOCATOR_BYTES, LocatorSalt, SigningKey, VerificationStatus,
    derive_locator,
};
use genomark::{GenoMark, Payload};
use genotrace::nulltest::NullTestTable;
use genotrace::result::{BindingKind, RecordingAssociationStatus};
use genotrace::{FileTrustStore, VerifyOptions, verify};
use serde_json::Value;

const RATE: u32 = 44_100;
/// Three GenoMark blocks at 44.1 kHz, so two disjoint blocks can agree and the class is `strong`.
const SECONDS: f64 = 29.0;

const POC_MANIFEST: &str =
    include_str!("../../audio-provenance-manifest/tests/fixtures/poc_signed_manifest.json");

const GENOMARK_NULL_TEST: &str = r#"{
  "schema": "audio-provenance-bench/1",
  "codec": {"name": "genomark-lepqim-v1", "description": "", "is_bench_fixture": false, "payload_len": 7},
  "totals": {
    "false_positive_trials": 370,
    "false_positive_accepts": 0,
    "overall_false_positive_rate": 0.0,
    "false_positive_upper_bound_95": 0.008108108108108109
  }
}"#;

fn key() -> SigningKey {
    SigningKey::from_raw_bytes(&[7u8; 32]).unwrap()
}

/// Broadband with transients and real energy in the 861-4307 Hz band, which is where the mark lives.
fn audio(seconds: f64) -> AudioBuffer {
    let frames = (seconds * f64::from(RATE)) as usize;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut samples = Vec::with_capacity(frames);
    for index in 0..frames {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let noise = ((state >> 33) as f64 / f64::from(u32::MAX)) * 2.0 - 1.0;
        let t = index as f64 / f64::from(RATE);
        let tau = 2.0 * std::f64::consts::PI;
        let tone = (tau * (1100.0 + 600.0 * (t * 0.7).sin()) * t).sin()
            + 0.7 * (tau * (1900.0 + 400.0 * (t * 1.1).cos()) * t).sin()
            + 0.5 * (tau * 3100.0 * t).sin();
        let envelope = 0.4 + 0.6 * (t * 3.0).sin().abs();
        let click = if index % 3701 < 40 { 0.6 } else { 0.0 };
        samples.push(((tone * envelope * 0.22) + noise * 0.05 + click * noise) as f32);
    }
    AudioBuffer::from_channels(RATE, &[samples]).unwrap()
}

fn marked(locator: [u8; LOCATOR_BYTES]) -> Vec<u8> {
    let payload = Payload::with_locator_bytes(GENOMARK_PAYLOAD_VERSION, 0, &locator).unwrap();
    let embedded = GenoMark::public().embed(&audio(SECONDS), payload).unwrap();
    wav::encode(&embedded, BitDepth::Int16).unwrap()
}

/// The POC fixture with `export` removed, a `locator_salt` added, and re-signed under our key.
///
/// `c2pa_claim` goes with `export` because the invariant port requires the claim's binding to cover
/// the hashed export bytes, which is not a statement a session with no export can make.
fn manifest_without_hard_binding(salt: LocatorSalt) -> Vec<u8> {
    let mut value: Value = serde_json::from_str(POC_MANIFEST).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("export");
    object.remove("c2pa_claim");
    object.remove("portable_signature");
    object.remove("manifest_signature");
    object.insert("locator_salt".to_string(), Value::String(salt.to_hex()));
    // This historical POC fixture predates explicit host-bypass telemetry. A current producer must
    // supply the counter before it may claim complete observation; make this synthetic fixture a
    // current record before re-signing it below.
    object["observation_coverage"]["counters"]["bypassed_buffers"] = Value::from(0u8);

    let signature = key().sign_manifest(&value, "signer.pub").unwrap();
    value.as_object_mut().unwrap().insert(
        "portable_signature".to_string(),
        serde_json::to_value(&signature).unwrap(),
    );
    serde_json::to_vec(&value).unwrap()
}

fn trust_store() -> FileTrustStore {
    let json = format!(
        r#"{{"format":"audio-provenance-trust-store-v0","anchors":[{{"signer_id":"{}","identity":"Signal Room Studios","authority":"studio-ca"}}]}}"#,
        key().signer_id()
    );
    FileTrustStore::from_json(json.as_bytes()).unwrap()
}

fn verify_marked_with(locator: [u8; LOCATOR_BYTES]) -> genotrace::VerifyResult {
    let dir = tempfile::tempdir().unwrap();
    let salt = LocatorSalt::from_bytes([3u8; 16]);
    let path = dir.path().join("mix.wav");
    std::fs::write(&path, marked(locator)).unwrap();
    std::fs::write(
        dir.path().join("mix.wav.audio-provenance.json"),
        manifest_without_hard_binding(salt),
    )
    .unwrap();

    let store = trust_store();
    let table = NullTestTable::from_bench_report(GENOMARK_NULL_TEST.as_bytes()).unwrap();
    verify(
        &path,
        &VerifyOptions::new()
            .with_trust_store(&store)
            .with_null_test(&table),
    )
    .unwrap()
}

fn our_locator() -> [u8; LOCATOR_BYTES] {
    derive_locator(
        &key().public_key_bytes(),
        &LocatorSalt::from_bytes([3u8; 16]),
    )
}

/// The arm is live: a record declaring no hard binding, carrying a mark that names it, verifies on
/// the soft binding alone. This is the surface the locator term guards, demonstrated rather than
/// derived.
#[test]
fn a_mark_that_names_this_record_substitutes_for_the_hard_binding_it_never_declared() {
    let result = verify_marked_with(our_locator());

    assert_eq!(result.status, VerificationStatus::Verified, "{result:?}");
    assert_eq!(result.reason, "soft_binding_accepted");
    assert_eq!(result.binding.kind, BindingKind::SoftGenomark);
    assert!(result.mark_recovery.recovered);
    assert!(!result.mark_recovery.located);
    assert!(!result.mark_recovery.recovered_via_mark);
    assert!(result.mark_recovery.record_locator_matched);
    assert_eq!(
        result.recording_association.status,
        RecordingAssociationStatus::Associated
    );
    assert!(result.recording_association.established);
    assert_eq!(result.identity.as_deref(), Some("Signal Room Studios"));
    // Every soft basis is capped below the hard binding's 1.0.
    assert!(result.r#match < 1.0, "{}", result.r#match);
}

/// The same record, the same anchored signer, a mark anyone can mint that names a DIFFERENT record.
/// It is evidence about that other record and none about this one.
#[test]
fn a_mark_that_names_another_record_is_not_evidence_about_this_one() {
    let mut other = our_locator();
    other[0] ^= 0xFF;
    let result = verify_marked_with(other);

    assert_eq!(result.status, VerificationStatus::Changed, "{result:?}");
    assert_eq!(result.reason, "no_binding_evidence");
    assert_eq!(result.binding.kind, BindingKind::None);
    assert_eq!(result.r#match, 0.0);
    assert!(result.mark_recovery.recovered);
    assert!(!result.mark_recovery.record_locator_matched);
    assert_eq!(
        result.recording_association.status,
        RecordingAssociationStatus::Rejected
    );
    assert!(!result.recording_association.established);
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.code == "recovered_mark_names_another_record"),
        "{:?}",
        result.findings
    );
}
