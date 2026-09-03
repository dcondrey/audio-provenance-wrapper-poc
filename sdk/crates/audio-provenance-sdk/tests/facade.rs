#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! Each test here pins one invariant the facade exists to hold, not a getter.

use std::path::{Path, PathBuf};

use audio_provenance_audio::{AudioBuffer, BitDepth, DecodeLimits, wav};
use audio_provenance_registry::{
    ContentHash, Fingerprint, Lookup, MarkId, MarkMatches, RecordId, RegistryBackend, RegistryKind,
    RegistryRecord, RegistrySource, ScoredCandidate, UnavailableKind,
};
use audio_provenance_sdk::{
    EmbedOptions, MarkPlan, SdkError, SidecarOutput, SignOptions, SignResult, SigningKey,
    VerificationStatus, VerifyOptions, capabilities, embed, sign, verify,
};

const SEED: [u8; 32] = [7u8; 32];
const SIGNED_AT: &str = "2026-03-14T09:30:00Z";

fn key() -> SigningKey {
    SigningKey::from_raw_bytes(&SEED).unwrap()
}

/// Two seconds of a decaying tone: long enough to decode, short enough that the GenoMark rung's
/// detector cannot dominate the test run.
fn write_wav(path: &Path) {
    write_tone(path, 2.0);
}

fn write_tone(path: &Path, seconds: f32) {
    let rate = 44_100u32;
    let frames = (rate as f32 * seconds) as usize;
    let samples: Vec<f32> = (0..frames)
        .map(|n| {
            let t = n as f32 / rate as f32;
            0.4 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                + 0.2 * (2.0 * std::f32::consts::PI * 1310.0 * t).sin()
                + 0.1 * (2.0 * std::f32::consts::PI * 2790.0 * t).sin()
        })
        .collect();
    let buffer = AudioBuffer::from_channels(rate, &[samples]).unwrap();
    std::fs::write(path, wav::encode(&buffer, BitDepth::Int16).unwrap()).unwrap();
}

/// Long enough for the GenoMark embedder, whose block is 9.66 s at 44.1 kHz.
fn write_long_wav(path: &Path) {
    write_tone(path, 20.0);
}

fn trust_store(dir: &Path, result: &SignResult) -> PathBuf {
    let path = dir.join("trust.json");
    let document = serde_json::json!({
        "format": "audio-provenance-trust-store-v0",
        "anchors": [{
            "signer_id": result.signer_id,
            "identity": "Signal Room Studios",
            "authority": "studio-ca",
        }],
    });
    std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
    path
}

/// The single most important type decision in the crate: a valid self-generated signature proves
/// KEY POSSESSION, so it yields no name. A name appears only once an anchor resolves one, and the
/// proof level moves with it.
#[test]
fn identity_is_none_until_an_anchor_resolves_one() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("master.wav");
    write_wav(&asset);

    let signed = sign(
        &asset,
        &SignOptions::new(key(), SIGNED_AT)
            .unwrap()
            .public_key_file("test.pub"),
    )
    .unwrap();
    assert_eq!(signed.signed_date, "2026-03-14");
    assert!(signed.asset_binds_content_sha256);

    let unanchored = verify(&asset, &VerifyOptions::new()).unwrap();
    assert_eq!(unanchored.status, VerificationStatus::Untrusted);
    assert_eq!(unanchored.identity, None);
    assert_eq!(unanchored.identity_authority, None);
    assert_eq!(
        unanchored.identity_proof_level.as_str(),
        "unknown_unobserved"
    );
    assert!(
        (unanchored.r#match - 1.0).abs() < f64::EPSILON,
        "a hard binding recomputed exactly, so match is 1.0: {}",
        unanchored.r#match
    );

    let store = trust_store(dir.path(), &signed);
    let anchored = verify(
        &asset,
        &VerifyOptions::new().with_trust_store_path(&store).unwrap(),
    )
    .unwrap();
    assert_eq!(anchored.status, VerificationStatus::Verified);
    assert_eq!(anchored.identity.as_deref(), Some("Signal Room Studios"));
    assert_eq!(anchored.identity_authority.as_deref(), Some("studio-ca"));
    assert_eq!(
        anchored.identity_proof_level.as_str(),
        "externally_verified"
    );
    assert_eq!(anchored.signed_at.as_deref(), Some("2026-03-14"));
    assert!(!anchored.incomplete);

    // The brief's field names are the serialisation contract the CLI's --json mode reads.
    let json: serde_json::Value = serde_json::to_value(&anchored).unwrap();
    assert_eq!(json["status"], "verified");
    assert_eq!(json["identity"], "Signal Room Studios");
    assert_eq!(json["signed_at"], "2026-03-14");
    assert_eq!(json["match"], 1.0);
}

#[derive(Debug)]
struct OutageBackend(RegistrySource);

impl OutageBackend {
    fn new() -> Self {
        Self(RegistrySource::new(
            "public",
            RegistryKind::Http,
            "https://registry.invalid",
        ))
    }

    fn outage<T>() -> Lookup<T> {
        Lookup::unavailable(UnavailableKind::Transport, "connection refused")
    }
}

impl RegistryBackend for OutageBackend {
    fn source(&self) -> &RegistrySource {
        &self.0
    }
    fn lookup_by_mark(&self, _mark: &MarkId) -> Lookup<MarkMatches> {
        Self::outage()
    }
    fn lookup_by_content_hash(&self, _hash: &ContentHash) -> Lookup<RegistryRecord> {
        Self::outage()
    }
    fn nearest_by_fingerprint(
        &self,
        _query: &Fingerprint,
        _limit: usize,
    ) -> Lookup<Vec<ScoredCandidate>> {
        Self::outage()
    }
    fn fetch(&self, _record: &RecordId) -> Lookup<RegistryRecord> {
        Self::outage()
    }
}

/// A network fault must never be readable as "this work is unregistered". The control arm proves
/// the same file really does return `not_found` when the search actually completes.
#[test]
fn a_registry_outage_is_an_error_and_never_a_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("bare.wav");
    write_wav(&asset);

    let completed = verify(&asset, &VerifyOptions::new()).unwrap();
    assert_eq!(completed.status, VerificationStatus::NotFound);
    assert!(!completed.incomplete);

    let options = VerifyOptions::new().with_registry(std::sync::Arc::new(OutageBackend::new()));
    match verify(&asset, &options) {
        Err(SdkError::RegistryUnavailable { method, detail }) => {
            assert_eq!(method, "content_hash_lookup");
            assert!(detail.contains("connection refused"), "{detail}");
        }
        other => panic!("an outage must not become a verdict: {other:?}"),
    }

    // `inspect` reports rather than raises: there is no verdict for an outage to contaminate.
    let report = audio_provenance_sdk::inspect(&asset, &options).unwrap();
    assert!(report.incomplete);
}

/// A manifest written into the asset is inside the bytes its own `content_sha256` covers, so that
/// digest can never recompute. The decoded-audio digest still does, and the fact that the container
/// moved is reported rather than swallowed.
#[test]
fn an_embedded_record_binds_on_decoded_audio_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("master.wav");
    let release = dir.path().join("release.wav");
    write_wav(&master);

    let signed = sign(
        &master,
        &SignOptions::new(key(), SIGNED_AT)
            .unwrap()
            .out(release.clone())
            .embed_manifest(true)
            .sidecar(SidecarOutput::None),
    )
    .unwrap();
    assert!(!signed.asset_binds_content_sha256);
    assert_eq!(
        signed.embedded_in.as_deref(),
        Some(release.to_str().unwrap())
    );

    let store = trust_store(dir.path(), &signed);
    let result = verify(
        &release,
        &VerifyOptions::new().with_trust_store_path(&store).unwrap(),
    )
    .unwrap();

    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(
        result.method,
        Some(audio_provenance_sdk::RecoveryMethod::EmbeddedManifest)
    );
    assert_eq!(
        result.binding.kind,
        audio_provenance_sdk::BindingKind::HardHash
    );
    assert_ne!(
        result.content_sha256, signed.content_sha256,
        "appending the record moved the file bytes"
    );
    assert_eq!(
        result.decoded_audio_sha256.as_deref(),
        Some(signed.decoded_audio_sha256.as_str()),
        "appending the record moved no sample"
    );
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.code == "container_bytes_changed"),
        "the container rewrite must be reported: {:?}",
        result.findings
    );
}

/// Marking changes the audio a hard binding covers, so it cannot follow a signature.
#[test]
fn marking_a_signed_asset_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("master.wav");
    write_wav(&asset);

    sign(&asset, &SignOptions::new(key(), SIGNED_AT).unwrap()).unwrap();

    let plan = MarkPlan::allocate(&key(), 0).unwrap();
    let outcome = embed(
        &asset,
        &EmbedOptions::new(dir.path().join("marked.wav"), plan.payload()),
    );
    assert!(
        matches!(outcome, Err(SdkError::MarkAfterSign { .. })),
        "{outcome:?}"
    );
}

/// A `--sidecar=` pointing at nothing is a miss, not an incomplete search. The distinction is the
/// difference between the CLI's usage exit code and its "could not finish" exit code.
#[test]
fn a_missing_explicit_sidecar_is_a_miss_and_not_a_raised_error() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("bare.wav");
    write_wav(&asset);

    let result = verify(
        &asset,
        &VerifyOptions::new().with_sidecar(audio_provenance_sdk::SidecarPolicy::Explicit(
            dir.path().join("absent.audio-provenance.json"),
        )),
    )
    .unwrap();
    assert_eq!(result.status, VerificationStatus::NotFound);
    assert!(!result.incomplete);
}

/// The one capability claim the docs forbid softening. `None` would read as "not yet measured".
#[test]
fn acoustic_rerecording_is_reported_as_the_literal_unsupported() {
    let reported = capabilities();
    assert_eq!(reported.acoustic_rerecording, "unsupported");
    assert_eq!(reported.mark.acoustic_rerecording, "unsupported");
    assert_eq!(reported.mark.time_stretch, "unsupported");
    assert_eq!(reported.mark.measured_transparency, None);
    assert_eq!(reported.mark.measured_survival, None);
    assert!(!reported.fingerprint.can_reach_verified);

    let json = serde_json::to_value(&reported).unwrap();
    assert_eq!(json["acoustic_rerecording"], "unsupported");
    assert_eq!(json["mark"]["acoustic_rerecording"], "unsupported");
    assert!(json["mark"]["measured_transparency"].is_null());
    assert_eq!(json["soft_binding_verified_requires_null_test"], true);
}

/// The producer path has to end somewhere a verifier can actually read. A record published to a
/// filesystem registry is recovered by the exact-content-hash rung with no sidecar and no embedded
/// copy in play.
#[test]
fn a_published_record_is_recovered_from_the_registry_alone() {
    let dir = tempfile::tempdir().unwrap();
    let asset = dir.path().join("master.wav");
    write_wav(&asset);

    let signed = sign(
        &asset,
        &SignOptions::new(key(), SIGNED_AT)
            .unwrap()
            .sidecar(SidecarOutput::None),
    )
    .unwrap();
    assert!(signed.sidecar_path.is_none());

    let registry =
        audio_provenance_sdk::LocalRegistryBackend::init("local", &dir.path().join("registry"))
            .unwrap();
    let receipt = audio_provenance_sdk::publish(&registry, &signed).unwrap();
    assert_eq!(receipt.record_id, signed.record_id);
    assert_eq!(receipt.locator, signed.locator);

    let store = trust_store(dir.path(), &signed);
    let options = VerifyOptions::new()
        .with_trust_store_path(&store)
        .unwrap()
        .with_registry(std::sync::Arc::new(registry));
    let result = verify(&asset, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(
        result.method,
        Some(audio_provenance_sdk::RecoveryMethod::ContentHashLookup)
    );
    assert_eq!(
        result.registry.as_ref().map(|source| source.kind),
        Some("filesystem")
    );
}

/// The two-phase publish, end to end on real audio: allocate a locator, embed it, then sign the
/// MARKED file with the salt that locator came from. What the file carries and what the record
/// answers to have to be one value, and it is checked the way GenoTrace checks it: blindly.
#[test]
fn an_embedded_mark_resolves_the_record_signed_over_it() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("master.wav");
    let marked = dir.path().join("marked.wav");
    write_long_wav(&master);

    let plan = MarkPlan::allocate(&key(), 0).unwrap();
    let report = embed(&master, &EmbedOptions::new(marked.clone(), plan.payload())).unwrap();

    let signed = sign(
        &marked,
        &SignOptions::new(key(), SIGNED_AT)
            .unwrap()
            .with_mark_plan(&plan)
            .unwrap()
            .sidecar(SidecarOutput::None),
    )
    .unwrap();

    assert_eq!(report.locator_hex, plan.locator_hex());
    assert_eq!(signed.locator, plan.locator_hex());
    assert_eq!(signed.locator_salt, plan.salt().to_hex());
    assert_eq!(report.namespace, 0);
    assert_eq!(report.payload_bits, 56);
    assert!(report.blocks >= 1, "{report:?}");

    let registry =
        audio_provenance_sdk::LocalRegistryBackend::init("local", &dir.path().join("registry"))
            .unwrap();
    let receipt = audio_provenance_sdk::publish(&registry, &signed).unwrap();
    assert_eq!(receipt.locator, plan.locator_hex());

    let written = wav::decode(&std::fs::read(&marked).unwrap(), &DecodeLimits::default()).unwrap();
    let detection = audio_provenance_sdk::GenoMark::public()
        .detect(&written)
        .unwrap();
    let payload = detection
        .payload()
        .expect("the mark just written must decode from the file it was written to");
    assert_eq!(hex::encode(payload.locator_bytes()), plan.locator_hex());

    let mark = MarkId::new(
        payload.version(),
        payload.namespace(),
        payload.locator_bytes(),
    )
    .unwrap();
    assert!(
        matches!(registry.lookup_by_mark(&mark), Lookup::Found(_)),
        "the recovered payload must resolve the record that was signed over it"
    );
}

/// A plan allocated under one key, signed with another, would publish a record no mark points at.
#[test]
fn signing_with_a_key_the_locator_was_not_allocated_under_is_refused() {
    let other = SigningKey::from_raw_bytes(&[9u8; 32]).unwrap();
    let plan = MarkPlan::allocate(&other, 0).unwrap();
    let outcome = SignOptions::new(key(), SIGNED_AT)
        .unwrap()
        .with_mark_plan(&plan);
    assert!(
        matches!(outcome, Err(SdkError::LocatorKeyMismatch { .. })),
        "{:?}",
        outcome.err()
    );
}

/// Re-signing an asset that already carries a record must leave exactly one, and it must be the
/// new one. The writer appends the chunk while `find_embedded` returns the FIRST match, so a
/// writer that does not remove the old chunk publishes a file that verifies against the manifest
/// it was supposed to replace.
#[test]
fn re_signing_an_embedded_asset_replaces_the_record_rather_than_shadowing_it() {
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("master.wav");
    let first = dir.path().join("first.wav");
    let second = dir.path().join("second.wav");
    write_wav(&master);

    let one = sign(
        &master,
        &SignOptions::new(key(), SIGNED_AT)
            .unwrap()
            .out(first.clone())
            .embed_manifest(true)
            .sidecar(SidecarOutput::None),
    )
    .unwrap();
    let two = sign(
        &first,
        &SignOptions::new(key(), "2027-05-02T11:00:00Z")
            .unwrap()
            .out(second.clone())
            .embed_manifest(true)
            .sidecar(SidecarOutput::None),
    )
    .unwrap();
    assert_ne!(one.record_id, two.record_id);

    let store = trust_store(dir.path(), &two);
    let result = verify(
        &second,
        &VerifyOptions::new().with_trust_store_path(&store).unwrap(),
    )
    .unwrap();

    assert_eq!(
        result.signed_at.as_deref(),
        Some("2027-05-02"),
        "the reader recovered the superseded record"
    );
    assert_eq!(
        std::fs::read(&second)
            .unwrap()
            .windows(4)
            .filter(|window| *window == b"aprv")
            .count(),
        1,
        "the asset carries more than one provenance chunk"
    );
}
