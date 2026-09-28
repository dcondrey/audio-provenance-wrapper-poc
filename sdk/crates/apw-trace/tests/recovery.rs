#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! End-to-end ladder behaviour on real bytes: a real WAV, a real Ed25519 signature, a real
//! canonical manifest.

use std::path::Path;

use audio_provenance_audio::{AudioBuffer, BitDepth, wav};
use audio_provenance_core::{SigningKey, VerificationStatus};
use audio_provenance_manifest::{HardBinding, ManifestDraft};
use audio_provenance_registry::{
    ContentHash, Fingerprint, Lookup, MarkId, MarkMatches, RecordId, RegistryBackend, RegistryKind,
    RegistryRecord, RegistrySource, ScoredCandidate, SignedAt, Unavailable, UnavailableKind,
};
use apw_trace::{
    RevocationStatus,
    FileTrustStore, InferredAssociationPolicy, NoTrustAnchors, RecoveryMethod, StepOutcome,
    VerifyOptions, decoded_audio_sha256, verify,
};

const SIGNER_SEED: [u8; 32] = [7u8; 32];

fn tone(seconds: f64) -> AudioBuffer {
    let rate = 44_100u32;
    let frames = (seconds * f64::from(rate)) as usize;
    let samples: Vec<f32> = (0..frames)
        .map(|index| {
            let t = index as f64 / f64::from(rate);
            (0.2 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as f32
        })
        .collect();
    AudioBuffer::from_channels(rate, &[samples]).unwrap()
}

fn wav_bytes(seconds: f64) -> Vec<u8> {
    wav::encode(&tone(seconds), BitDepth::Int16).unwrap()
}

/// Signs a Audio Provenance manifest binding the given bytes, and returns its canonical form.
fn sign_manifest(container: &[u8]) -> (Vec<u8>, String) {
    let key = SigningKey::from_raw_bytes(&SIGNER_SEED).unwrap();
    let audio = audio_provenance_audio::decode_bytes(
        container,
        &audio_provenance_audio::DecodeLimits::default(),
    )
    .unwrap();
    let binding = HardBinding::new(
        &audio_provenance_core::sha256_hex(container),
        Some(container.len() as u64),
        Some(&decoded_audio_sha256(&audio)),
    )
    .unwrap();
    let draft = ManifestDraft::new("2026-03-13", binding)
        .unwrap()
        .with_locator_salt(audio_provenance_core::LocatorSalt::from_bytes([5u8; 16]));
    let signature = key
        .sign_manifest(&draft.to_unsigned_value().unwrap(), "signer.pub")
        .unwrap();
    (draft.seal(&signature).unwrap(), key.signer_id())
}

fn trust_store(signer_id: &str) -> FileTrustStore {
    let json = format!(
        r#"{{"format":"audio-provenance-trust-store-v0","anchors":[{{"signer_id":"{signer_id}","identity":"Signal Room Studios","authority":"studio-ca"}}]}}"#
    );
    FileTrustStore::from_json(json.as_bytes()).unwrap()
}

/// A registry that is reachable but empty.
#[derive(Debug)]
struct EmptyRegistry(RegistrySource);

impl EmptyRegistry {
    fn new() -> Self {
        Self(RegistrySource::new("test", RegistryKind::Local, "memory"))
    }
}

impl RegistryBackend for EmptyRegistry {
    fn source(&self) -> &RegistrySource {
        &self.0
    }
    fn lookup_by_mark(&self, _mark: &MarkId) -> Lookup<MarkMatches> {
        Lookup::NotFound
    }
    fn lookup_by_content_hash(&self, _hash: &ContentHash) -> Lookup<RegistryRecord> {
        Lookup::NotFound
    }
    fn nearest_by_fingerprint(
        &self,
        _query: &Fingerprint,
        _limit: usize,
    ) -> Lookup<Vec<ScoredCandidate>> {
        Lookup::NotFound
    }
    fn fetch(&self, _record: &RecordId) -> Lookup<RegistryRecord> {
        Lookup::NotFound
    }
}

/// A registry that is down. Every arm is an outage, never a miss.
#[derive(Debug)]
struct DownRegistry(RegistrySource);

impl DownRegistry {
    fn new() -> Self {
        Self(RegistrySource::new("test", RegistryKind::Http, "https://x"))
    }
    fn outage<T>() -> Lookup<T> {
        Lookup::Unavailable(Unavailable::new(
            UnavailableKind::Transport,
            "connection refused",
        ))
    }
}

impl RegistryBackend for DownRegistry {
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

/// A registry that answers EVERY mark query with the one record it holds, whatever was asked for.
/// This is the substituting registry the locator re-derivation exists to catch.
#[derive(Debug)]
struct SubstitutingRegistry {
    source: RegistrySource,
    record: RegistryRecord,
}

impl RegistryBackend for SubstitutingRegistry {
    fn source(&self) -> &RegistrySource {
        &self.source
    }
    fn lookup_by_mark(&self, _mark: &MarkId) -> Lookup<MarkMatches> {
        match MarkMatches::new(vec![self.record.clone()]) {
            Ok(matches) => Lookup::Found(matches),
            Err(error) => Lookup::Unavailable(Unavailable::new(
                UnavailableKind::IndexCorrupt,
                error.to_string(),
            )),
        }
    }
    fn lookup_by_content_hash(&self, _hash: &ContentHash) -> Lookup<RegistryRecord> {
        Lookup::NotFound
    }
    fn nearest_by_fingerprint(
        &self,
        _query: &Fingerprint,
        _limit: usize,
    ) -> Lookup<Vec<ScoredCandidate>> {
        Lookup::NotFound
    }
    fn fetch(&self, _record: &RecordId) -> Lookup<RegistryRecord> {
        Lookup::NotFound
    }
}

/// Broadband, because a mark needs energy across 861-4307 Hz to carry and a pure tone punctures
/// nearly every slot. Two blocks of 9.66 s is the shortest input the detector is specified for.
fn noise(seconds: f64) -> AudioBuffer {
    let rate = 44_100u32;
    let frames = (seconds * f64::from(rate)) as usize;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let samples: Vec<f32> = (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 40) as f32 / 8_388_608.0 - 1.0) * 0.25
        })
        .collect();
    AudioBuffer::from_channels(rate, &[samples]).unwrap()
}

/// THE property the two-phase publish rests on. A mark resolves only to a record whose own signer
/// intended it: the ladder re-derives the locator from the RECEIVED bytes' own declared public key
/// and locator_salt, never from the backend's index key. A registry that answers with a genuine,
/// validly signed record that does not derive the requested locator is refused, terminally, with no
/// descent to a lower rung.
#[test]
fn a_registry_answering_with_a_record_that_does_not_derive_the_locator_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let key = SigningKey::from_raw_bytes(&SIGNER_SEED).unwrap();

    // The mark carries a locator nothing derives from this signer's key.
    let payload = apw_watermark::Payload::with_locator_bytes(1, 0, &[0xab; 6]).unwrap();
    let marked = apw_watermark::Watermark::public()
        .embed(&noise(21.0), payload)
        .unwrap();
    let container = wav::encode(&marked, BitDepth::Int24).unwrap();
    let path = dir.path().join("marked.wav");
    std::fs::write(&path, &container).unwrap();

    // The record is entirely honest: signed by this key, binding these exact bytes, carrying a salt.
    // Its only fault is that it does not derive the locator the mark asked for.
    let (manifest_bytes, _) = sign_manifest(&container);
    let record = RegistryRecord::from_signed_manifest(
        serde_json::from_slice(&manifest_bytes).unwrap(),
        ContentHash::from_content(&container),
        None,
        SignedAt::parse("2026-03-13T00:00:00Z").unwrap(),
    )
    .unwrap();
    assert_ne!(record.mark_id().locator(), payload.locator_bytes());

    let registry = SubstitutingRegistry {
        source: RegistrySource::new("hostile", RegistryKind::Http, "https://x"),
        record,
    };
    let store = trust_store(&key.signer_id());
    let result = verify(
        &path,
        &VerifyOptions::new()
            .with_registry(&registry)
            .with_trust_store(&store)
            .with_sidecar(apw_trace::SidecarPolicy::Disabled),
    )
    .unwrap();

    assert_eq!(result.status, VerificationStatus::Untrusted);
    assert_eq!(result.reason, "locator_mismatch");
    // Terminal: the anchored signer is never named on a record the ladder refused to admit.
    assert!(result.identity.is_none(), "{:?}", result.identity);
}

fn write_asset(dir: &Path, name: &str, seconds: f64) -> (std::path::PathBuf, Vec<u8>) {
    let bytes = wav_bytes(seconds);
    let path = dir.join(name);
    std::fs::write(&path, &bytes).unwrap();
    (path, bytes)
}

/// The whole happy path: a sidecar manifest whose signed hard binding recomputes over the file, and
/// a signer the trust store anchors.
#[test]
fn a_sidecar_manifest_with_an_anchored_signer_verifies_at_one() {
    let dir = tempfile::tempdir().unwrap();
    let (path, bytes) = write_asset(dir.path(), "mix.wav", 1.0);
    let (manifest, signer_id) = sign_manifest(&bytes);
    std::fs::write(dir.path().join("mix.wav.audio-provenance.json"), &manifest).unwrap();

    let store = trust_store(&signer_id);
    let registry = EmptyRegistry::new();
    let options = VerifyOptions::new()
        .with_trust_store(&store)
        .with_registry(&registry);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(result.r#match, 1.0);
    assert_eq!(result.identity.as_deref(), Some("Signal Room Studios"));
    assert_eq!(result.identity_authority.as_deref(), Some("studio-ca"));
    // A flat store declares no revocation list, so the name carries an explicit unchecked marker
    // and finding rather than an implied clean bill.
    assert_eq!(result.revocation_status, RevocationStatus::RevocationUnchecked);
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.code == "revocation_unchecked")
    );
    assert_eq!(result.method, Some(RecoveryMethod::SidecarManifest));
    assert_eq!(result.signed_at.as_deref(), Some("2026-03-13"));
    assert!(!result.incomplete);
}

/// The same manifest with no anchor configured. The signature is cryptographically perfect and the
/// audio is untouched; the honest answer is still `untrusted` with no name.
#[test]
fn a_perfect_signature_without_an_anchor_is_untrusted_and_unnamed() {
    let dir = tempfile::tempdir().unwrap();
    let (path, bytes) = write_asset(dir.path(), "demo.wav", 1.0);
    let (manifest, _) = sign_manifest(&bytes);
    std::fs::write(dir.path().join("demo.wav.audio-provenance.json"), &manifest).unwrap();

    let anchors = NoTrustAnchors;
    let options = VerifyOptions::new().with_trust_store(&anchors);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Untrusted);
    assert_eq!(result.reason, "trust_anchor_unresolved");
    assert!(result.identity.is_none());
    assert!(result.identity_authority.is_none());
    assert_eq!(result.revocation_status, RevocationStatus::NotApplicable);
    // The binding still recomputed; the status is about the signer, not the audio.
    assert_eq!(result.r#match, 1.0);
}

/// A manifest signed over different audio. The signature is valid, so this is `changed`, and a
/// resolved anchor may still name the signer.
#[test]
fn audio_that_moved_under_a_valid_signature_is_changed() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_asset(dir.path(), "mix.wav", 1.0);
    let other = wav_bytes(2.0);
    let (manifest, signer_id) = sign_manifest(&other);
    std::fs::write(dir.path().join("mix.wav.audio-provenance.json"), &manifest).unwrap();

    let store = trust_store(&signer_id);
    let options = VerifyOptions::new().with_trust_store(&store);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Changed);
    assert_eq!(result.reason, "hard_binding_mismatch");
    assert_eq!(result.identity.as_deref(), Some("Signal Room Studios"));
    assert_eq!(result.r#match, 0.0);
    assert!(result.binding.expected_sha256.is_some());
}

/// A registry outage is an error condition, never a provenance verdict. It sets `incomplete` and
/// leaves the status at `not_found`.
#[test]
fn a_registry_outage_is_not_found_plus_incomplete_and_never_a_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_asset(dir.path(), "bare.wav", 1.0);

    let registry = DownRegistry::new();
    let options = VerifyOptions::new().with_registry(&registry);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::NotFound);
    assert!(
        result.incomplete,
        "an outage must mark the search incomplete"
    );
    assert_eq!(result.reason, "recovery_incomplete");
    assert!(!result.recovery.exhausted);
    assert!(
        result
            .trace
            .iter()
            .any(|step| step.outcome == StepOutcome::Unavailable),
        "the outage must be visible in the trace"
    );
}

/// `--offline` skips the network rungs deliberately. A deliberate skip is not an outage, so it must
/// not set `incomplete`; CLI_SPEC's exit code 4 would otherwise fire on every offline run.
#[test]
fn offline_skips_registry_rungs_without_setting_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_asset(dir.path(), "bare.wav", 1.0);

    let registry = DownRegistry::new();
    let options = VerifyOptions::new().with_registry(&registry).offline(true);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::NotFound);
    assert!(!result.incomplete);
    assert_eq!(result.reason, "no_manifest_recovered");
    for step in &result.trace {
        assert_ne!(step.outcome, StepOutcome::Unavailable, "{step:?}");
    }
}

/// A corrupt provenance chunk is recorded and stepped over, never forced into a candidate that
/// would block the lower rungs.
#[test]
fn a_corrupt_embedded_slot_is_recorded_and_the_ladder_continues() {
    let dir = tempfile::tempdir().unwrap();
    let mut bytes = wav_bytes(1.0);
    // A `aprv` chunk whose body is not a manifest at all.
    let junk = b"not a manifest";
    bytes.extend_from_slice(b"aprv");
    bytes.extend_from_slice(&(junk.len() as u32).to_le_bytes());
    bytes.extend_from_slice(junk);
    let path = dir.path().join("tampered.wav");
    std::fs::write(&path, &bytes).unwrap();

    let (manifest, signer_id) = sign_manifest(&bytes);
    std::fs::write(
        dir.path().join("tampered.wav.audio-provenance.json"),
        &manifest,
    )
    .unwrap();

    let store = trust_store(&signer_id);
    let options = VerifyOptions::new().with_trust_store(&store);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(result.method, Some(RecoveryMethod::SidecarManifest));
    assert!(
        result
            .recovery
            .diagnostics
            .iter()
            .any(|finding| finding.code == "embedded_unparseable"),
        "the corrupt slot must be reported: {:?}",
        result.recovery.diagnostics
    );
}

/// The default policy for rung 5 emits no candidate at all, so a fingerprint hit cannot become any
/// status other than `not_found`.
#[test]
fn the_default_inferred_policy_emits_no_fingerprint_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let (path, _) = write_asset(dir.path(), "bare.wav", 1.0);
    let options = VerifyOptions::new();
    let result = verify(&path, &options).unwrap();
    assert_eq!(result.status, VerificationStatus::NotFound);

    let opted_in =
        VerifyOptions::new().with_inferred_association(InferredAssociationPolicy::EmitCandidate);
    let result = verify(&path, &opted_in).unwrap();
    assert_eq!(result.status, VerificationStatus::NotFound);
}

/// The second exact binding. Appending a chunk rewrites the container without touching a sample, so
/// the signed content digest stops recomputing and the signed decoded-audio digest still does. That
/// is `verified` at `match` 1.0, with the byte change reported rather than swallowed.
#[test]
fn a_container_only_edit_verifies_on_the_decoded_audio_binding() {
    let dir = tempfile::tempdir().unwrap();
    let original = wav_bytes(1.0);
    let (manifest, signer_id) = sign_manifest(&original);

    let mut edited = original.clone();
    let note = b"a rewritten metadata chunk";
    edited.extend_from_slice(b"LIST");
    edited.extend_from_slice(&(note.len() as u32).to_le_bytes());
    edited.extend_from_slice(note);
    assert_ne!(edited, original);

    let path = dir.path().join("retagged.wav");
    std::fs::write(&path, &edited).unwrap();
    std::fs::write(
        dir.path().join("retagged.wav.audio-provenance.json"),
        &manifest,
    )
    .unwrap();

    let store = trust_store(&signer_id);
    let options = VerifyOptions::new().with_trust_store(&store);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Verified);
    assert_eq!(result.reason, "hard_binding_decoded_audio_only");
    assert_eq!(result.r#match, 1.0);
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.code == "container_bytes_changed"),
        "the byte change must be reported: {:?}",
        result.findings
    );
}

/// A signature that does not verify is `untrusted`, never `changed`: `changed` is reserved for
/// audio that moved under a VALID signature. The reason is reported as a finding with its own code.
#[test]
fn an_invalid_signature_is_untrusted_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let (path, bytes) = write_asset(dir.path(), "mix.wav", 1.0);
    let (manifest, signer_id) = sign_manifest(&bytes);

    // One hex digit of the signature, flipped. The document stays canonical, so the only thing that
    // changed is whether the signature checks out.
    let text = String::from_utf8(manifest).unwrap();
    let marker = "\"signature_hex\":\"";
    let at = text.find(marker).unwrap() + marker.len();
    let mut tampered = text.clone();
    let original = tampered.as_bytes()[at];
    tampered.replace_range(at..at + 1, if original == b'a' { "b" } else { "a" });
    assert_ne!(tampered, text);
    std::fs::write(dir.path().join("mix.wav.audio-provenance.json"), &tampered).unwrap();

    // The most favourable trust available, so the verdict is the signature's doing.
    let store = trust_store(&signer_id);
    let options = VerifyOptions::new().with_trust_store(&store);
    let result = verify(&path, &options).unwrap();

    assert_eq!(result.status, VerificationStatus::Untrusted);
    assert_eq!(result.reason, "signature_invalid");
    assert!(result.identity.is_none());
    assert_eq!(result.r#match, 0.0);
    assert!(
        result
            .findings
            .iter()
            .any(|finding| finding.code == "signature_invalid" && finding.path == "$"),
        "the rejection must be reported with its own code: {:?}",
        result.findings
    );
}
