#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// A test asserts; the workspace bans these in production code, where a panic is a defect rather
// than the reporting mechanism.

//! Rung 5: the landmark index round-trips, the scorer re-derives a real alignment locally, an
//! uncorrelated query is rejected, and a fingerprint hit never verifies.

use audio_provenance_audio::{AudioBuffer, BitDepth, wav};
use audio_provenance_core::{LocatorSalt, SigningKey, VerificationStatus};
use audio_provenance_manifest::{AudioFingerprint, HardBinding, ManifestDraft};
use audio_provenance_registry::{
    ContentHash, Fingerprint, Lookup, MarkId, MarkMatches, RecordId, RegistryBackend, RegistryKind,
    RegistryRecord, RegistrySource, ScoredCandidate, SignedAt,
};
use apw_trace::fingerprint::index::FingerprintIndex;
use apw_trace::fingerprint::reference::{ReferenceFingerprint, compare};
use apw_trace::fingerprint::score::{MIN_PEAK, ScoreLimits};
use apw_trace::fingerprint::{FingerprintQuery, MemoryFingerprintIndex, reference_landmarks};
use apw_trace::{FileFingerprintIndex, InferredAssociationPolicy, VerifyOptions, verify};

const RATE: u32 = 44_100;

/// A deterministic broadband signal with transients, which is what a constellation needs: pure
/// tones give a handful of stationary peaks and no evidence at all.
///
/// Every structural parameter is derived from the seed, so two seeds give recordings that share no
/// event times. A fixture whose transients land at the same instants for every seed would make the
/// negative case pass for the wrong reason.
fn textured(seconds: f64, seed: u64) -> AudioBuffer {
    let frames = (seconds * f64::from(RATE)) as usize;
    let mut state = seed | 1;
    let spread = |n: u64| f64::from((seed.wrapping_mul(n) % 1000) as u32) / 1000.0;
    let (base, span) = (220.0 + 900.0 * spread(7), 400.0 + 900.0 * spread(13));
    let (wobble, second) = (0.3 + 2.0 * spread(29), 900.0 + 1600.0 * spread(31));
    let click_period = 2600 + (seed % 5000) as usize;
    let click_width = 20 + (seed % 60) as usize;
    let mut samples = Vec::with_capacity(frames);
    for index in 0..frames {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let noise = ((state >> 33) as f64 / f64::from(u32::MAX)) * 2.0 - 1.0;
        let t = index as f64 / f64::from(RATE);
        let sweep = (2.0 * std::f64::consts::PI * (base + span * (t * wobble).sin()) * t).sin()
            + 0.6 * (2.0 * std::f64::consts::PI * (second + 700.0 * (t * 1.3).cos()) * t).sin()
            + 0.4 * (2.0 * std::f64::consts::PI * (2300.0 + 400.0 * (t * 0.35).sin()) * t).sin();
        let envelope = 0.35 + 0.65 * ((t * (3.0 + 6.0 * spread(17))).sin().abs());
        let click = if index % click_period < click_width {
            0.8
        } else {
            0.0
        };
        samples.push(((sweep * envelope * 0.25) + noise * 0.05 + click * noise) as f32);
    }
    AudioBuffer::from_channels(RATE, &[samples]).unwrap()
}

fn build_index(track: u64, record: RecordId, audio: &AudioBuffer) -> MemoryFingerprintIndex {
    let landmarks = reference_landmarks(audio).unwrap();
    assert!(
        !landmarks.is_empty(),
        "the reference produced no landmarks at all"
    );
    let mut index = MemoryFingerprintIndex::new();
    index.insert_landmarks(track, record, &landmarks);
    index
}

fn record_id(byte: u8) -> RecordId {
    RecordId::parse_hex(&hex::encode([byte; 32])).unwrap()
}

/// The on-disk index is a different code path from the in-memory one: an offset table, bounded
/// bucket reads, and a track table. It must answer identically or the two can drift apart silently.
#[test]
fn the_file_index_answers_exactly_as_the_memory_index_does() {
    let audio = textured(8.0, 11);
    let record = record_id(0xAB);
    let index = build_index(1, record, &audio);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fp.idx");
    index.write_to(&path).unwrap();
    let on_disk = FileFingerprintIndex::open(&path).unwrap();

    assert_eq!(on_disk.track_count(), index.track_count());
    assert_eq!(on_disk.record_id(1), Some(record));

    let landmarks = reference_landmarks(&audio).unwrap();
    let mut compared = 0usize;
    for landmark in landmarks.iter().take(500) {
        let mut memory = index.postings_for(landmark.hash).unwrap();
        let mut file = on_disk.postings_for(landmark.hash).unwrap();
        memory.sort_unstable();
        file.sort_unstable();
        assert_eq!(memory, file, "hash {:#010x}", landmark.hash);
        compared += 1;
    }
    assert!(compared > 0);
}

/// A query of the indexed recording aligns at offset zero with a dominant peak, and the score is
/// re-derived from the returned postings rather than taken from anyone's word.
#[test]
fn an_indexed_recording_recovers_its_own_alignment() {
    let audio = textured(8.0, 11);
    let record = record_id(0x5C);
    let index = build_index(42, record, &audio);

    let query = FingerprintQuery::from_audio(&audio).unwrap();
    let search = apw_trace::fingerprint::search(&index, &query, ScoreLimits::default()).unwrap();

    let hit = search.hit.expect("a recording must match itself");
    assert_eq!(hit.record, record);
    assert_eq!(hit.rate, 1.0, "the 1.00 pass must succeed without a sweep");
    assert_eq!(hit.alignment.peak_offset_frames, 0);
    assert!(
        hit.alignment.peak >= MIN_PEAK,
        "peak {} is under the gate",
        hit.alignment.peak
    );
    assert!(hit.alignment.aligned_span_seconds >= 3.0);
    assert!((0.0..=1.0).contains(&hit.alignment.coherence_ratio));
    assert!(!search.budget_exceeded);
}

fn slice_audio(audio: &AudioBuffer, start_seconds: f64, end_seconds: f64) -> AudioBuffer {
    let start = (start_seconds * f64::from(RATE)) as usize;
    let end = ((end_seconds * f64::from(RATE)) as usize).min(audio.frames());
    AudioBuffer::from_channels(RATE, &[audio.channel(0).unwrap()[start..end].to_vec()]).unwrap()
}

fn substitute(
    original: &AudioBuffer,
    replacement: &AudioBuffer,
    start_seconds: f64,
    end_seconds: f64,
) -> AudioBuffer {
    let start = (start_seconds * f64::from(RATE)) as usize;
    let end = ((end_seconds * f64::from(RATE)) as usize).min(original.frames());
    let mut samples = original.channel(0).unwrap().to_vec();
    samples[start..end].copy_from_slice(&replacement.channel(0).unwrap()[start..end]);
    AudioBuffer::from_channels(RATE, &[samples]).unwrap()
}

/// Regression for the critical span-coverage defect: the first and last
/// landmarks still align across this replacement, so a global span reads 100%.
/// Local signed-region coverage must reject it while still accepting a crop.
#[test]
fn interior_substitution_cannot_hide_between_aligned_edges() {
    let original = textured(24.0, 0xA11CE);
    let unrelated = textured(24.0, 0xBAD5EED);
    let reference = ReferenceFingerprint::from_audio(&original)
        .unwrap()
        .unwrap();

    let changed = substitute(&original, &unrelated, 8.0, 12.0);
    let changed_query = FingerprintQuery::from_audio(&changed).unwrap();
    let rejection = compare(&reference, &changed_query, ScoreLimits::default()).unwrap();
    assert!(
        !rejection.affirmed,
        "four substituted interior seconds affirmed: {rejection:?}"
    );
    assert!(
        rejection.max_unexplained_gap_seconds > 0.0 || rejection.offset_discontinuities > 0,
        "the rejection did not surface a local gap or discontinuity: {rejection:?}"
    );

    let crop = slice_audio(&original, 3.37, 19.81);
    let crop_query = FingerprintQuery::from_audio(&crop).unwrap();
    let crop_affirmation = compare(&reference, &crop_query, ScoreLimits::default()).unwrap();
    assert!(
        crop_affirmation.affirmed,
        "an arbitrary legitimate crop was rejected: {crop_affirmation:?}"
    );
    assert!(crop_affirmation.duration_consistent);
}

/// Two recordings from `audio-provenance-bench`'s own corpus recipes, which were written for a watermark
/// bench with no knowledge of this scorer's gates. Neither may match the other.
///
/// The seeded generator above shares a writer with the code under test, so it can only ever show
/// that the gates are not trivially open. These two show it against material chosen by someone
/// else: transient-dense percussion against a stationary harmonic pad, the two content classes a
/// constellation is most and least suited to.
#[test]
fn independently_authored_recordings_do_not_match_each_other() {
    let drum = bench_drum(12.0);
    let pad = bench_pad(12.0);

    let index = build_index(1, record_id(0x01), &drum);
    let query = FingerprintQuery::from_audio(&pad).unwrap();
    let search = apw_trace::fingerprint::search(&index, &query, ScoreLimits::default()).unwrap();
    assert!(
        search.hit.is_none(),
        "a harmonic pad aligned against percussion: {:?}",
        search.hit
    );

    // The reverse direction too: a scorer that only rejects one ordering rejects nothing.
    let index = build_index(2, record_id(0x02), &pad);
    let query = FingerprintQuery::from_audio(&drum).unwrap();
    let search = apw_trace::fingerprint::search(&index, &query, ScoreLimits::default()).unwrap();
    assert!(
        search.hit.is_none(),
        "percussion aligned against a harmonic pad: {:?}",
        search.hit
    );

    // And the percussion still recovers itself, so the rejections above are not the scorer being
    // blind to this material.
    let index = build_index(3, record_id(0x03), &drum);
    let query = FingerprintQuery::from_audio(&drum).unwrap();
    let search = apw_trace::fingerprint::search(&index, &query, ScoreLimits::default()).unwrap();
    assert!(search.hit.is_some(), "percussion did not match itself");
}

/// A different seed of the same generator must not match either.
#[test]
fn an_uncorrelated_recording_does_not_match() {
    let indexed = textured(8.0, 11);
    let index = build_index(1, record_id(0x01), &indexed);

    let other = textured(8.0, 90_210);
    let query = FingerprintQuery::from_audio(&other).unwrap();
    let search = apw_trace::fingerprint::search(&index, &query, ScoreLimits::default()).unwrap();

    assert!(
        search.hit.is_none(),
        "an unrelated recording aligned: {:?}",
        search.hit
    );
}

/// `audio-provenance-bench`'s `drum` corpus recipe, transcribed from its ffmpeg `aevalsrc` expression at
/// crates/audio-provenance-bench/src/corpus.rs so the fixture needs no external binary.
fn bench_drum(seconds: f64) -> AudioBuffer {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    render(seconds, |t| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let noise = ((state >> 33) as f64 / f64::from(u32::MAX)) * 2.0 - 1.0;
        0.85 * (-28.0 * (t % 0.5)).exp() * (2.0 * std::f64::consts::PI * 58.0 * (t % 0.5)).sin()
            + 0.45
                * (-14.0 * (t % 1.0)).exp()
                * (2.0 * std::f64::consts::PI * 185.0 * (t % 1.0)).sin()
            + 0.22 * (-140.0 * (t % 0.25)).exp() * noise
    })
}

/// `audio-provenance-bench`'s `pad_left` corpus recipe, transcribed the same way.
fn bench_pad(seconds: f64) -> AudioBuffer {
    render(seconds, |t| {
        let tau = 2.0 * std::f64::consts::PI;
        0.24 * ((tau * 110.0 * t).sin()
            + 0.5 * (tau * 220.0 * t).sin()
            + 0.33 * (tau * 330.5 * t).sin()
            + 0.25 * (tau * 440.0 * t).sin()
            + 0.18 * (tau * 661.0 * t).sin())
            * (0.7 + 0.3 * (tau * 0.31 * t).sin())
    })
}

fn render(seconds: f64, mut sample: impl FnMut(f64) -> f64) -> AudioBuffer {
    let frames = (seconds * f64::from(RATE)) as usize;
    let samples: Vec<f32> = (0..frames)
        .map(|index| sample(index as f64 / f64::from(RATE)) as f32)
        .collect();
    AudioBuffer::from_channels(RATE, &[samples]).unwrap()
}

/// Under three seconds of query there is not enough evidence for any fingerprint answer, and the
/// rung says so rather than returning a weak one.
#[test]
fn a_short_query_is_refused_outright() {
    let indexed = textured(8.0, 11);
    let index = build_index(1, record_id(0x01), &indexed);

    let short = textured(2.0, 11);
    let query = FingerprintQuery::from_audio(&short).unwrap();
    let search = apw_trace::fingerprint::search(&index, &query, ScoreLimits::default()).unwrap();

    assert!(search.too_short);
    assert!(search.hit.is_none());
}

/// The posting budget aborts the rung instead of reporting a truncated scan as a weak result.
#[test]
fn the_posting_budget_aborts_rather_than_degrading_silently() {
    let audio = textured(8.0, 11);
    let index = build_index(1, record_id(0x01), &audio);
    let query = FingerprintQuery::from_audio(&audio).unwrap();

    let search =
        apw_trace::fingerprint::search(&index, &query, ScoreLimits { posting_budget: 5 }).unwrap();
    assert!(search.budget_exceeded);
    assert!(search.hit.is_none());
}

#[derive(Debug)]
struct OneRecordRegistry {
    source: RegistrySource,
    record: RegistryRecord,
}

impl RegistryBackend for OneRecordRegistry {
    fn source(&self) -> &RegistrySource {
        &self.source
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
    fn fetch(&self, record: &RecordId) -> Lookup<RegistryRecord> {
        if *record == self.record.record_id() {
            Lookup::Found(self.record.clone())
        } else {
            Lookup::NotFound
        }
    }
}

/// THE RUNG 5 INVARIANT, end to end. A fingerprint hit reaches a real manifest with a real
/// signature and an ANCHORED signer, and the result is still not `verified`: the recovered manifest
/// binds different audio, and a perceptual guess may never stand in for a hard binding.
#[test]
fn a_fingerprint_recovered_manifest_never_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let audio = textured(8.0, 11);
    let container = wav::encode(&audio, BitDepth::Int16).unwrap();
    let path = dir.path().join("ripped.wav");
    std::fs::write(&path, &container).unwrap();

    // The registered record was signed over DIFFERENT bytes, which is the honest shape of a
    // fingerprint recovery: the audio was re-encoded, so no exact digest survives.
    let signed_over = wav::encode(&textured(8.0, 12), BitDepth::Int16).unwrap();
    let key = SigningKey::from_raw_bytes(&[9u8; 32]).unwrap();
    let binding = HardBinding::new(
        &audio_provenance_core::sha256_hex(&signed_over),
        Some(signed_over.len() as u64),
        None,
    )
    .unwrap();
    let draft = ManifestDraft::new("2026-03-13", binding)
        .unwrap()
        .with_locator_salt(LocatorSalt::from_bytes([3u8; 16]));
    let signature = key
        .sign_manifest(&draft.to_unsigned_value().unwrap(), "signer.pub")
        .unwrap();
    let manifest_bytes = draft.seal(&signature).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
    let record = RegistryRecord::from_signed_manifest(
        manifest,
        ContentHash::from_content(&signed_over),
        None,
        SignedAt::parse("2026-03-13T00:00:00Z").unwrap(),
    )
    .unwrap();

    let index = build_index(7, record.record_id(), &audio);
    let registry = OneRecordRegistry {
        source: RegistrySource::new("test", RegistryKind::Local, "memory"),
        record,
    };
    let store = trust_store(&key.signer_id());

    let options = VerifyOptions::new()
        .with_registry(&registry)
        .with_fingerprint_index(&index)
        .with_trust_store(&store)
        .with_inferred_association(InferredAssociationPolicy::EmitCandidate);
    let result = verify(&path, &options).unwrap();

    assert_ne!(
        result.status,
        VerificationStatus::Verified,
        "a fingerprint recovery must never verify: {result:?}"
    );
    assert!(result.r#match < 1.0);
}

fn trust_store(signer_id: &str) -> apw_trace::FileTrustStore {
    let json = format!(
        r#"{{"format":"audio-provenance-trust-store-v0","anchors":[{{"signer_id":"{signer_id}","identity":"Signal Room Studios","authority":"studio-ca"}}]}}"#
    );
    apw_trace::FileTrustStore::from_json(json.as_bytes()).unwrap()
}

/// The reference constellation round-trips exactly, and every way of corrupting the blob is
/// refused.
///
/// This is a parsing boundary on attacker-chosen bytes: the blob rides inside a manifest, and a
/// signature proves only that the SIGNER chose it, which is no comfort when the signer is the
/// attacker. A decoder that accepted a non-ascending or out-of-band peak list would produce a
/// constellation no encoder could emit, and the comparison would then be scoring a shape the
/// scheme's own gates were never measured against.
#[test]
fn a_reference_constellation_round_trips_and_refuses_a_corrupt_blob() {
    let audio = textured(8.0, 4242);
    let reference = ReferenceFingerprint::from_audio(&audio).unwrap().unwrap();
    let declared = reference.to_manifest_fingerprint().unwrap();
    assert_eq!(declared.algorithm(), apw_trace::fingerprint::ALGORITHM_ID);
    assert!(reference.peak_count() > 0, "the fixture carries no peaks");

    let decoded = ReferenceFingerprint::from_manifest_fingerprint(&declared)
        .unwrap()
        .unwrap();
    assert_eq!(
        decoded.to_manifest_fingerprint().unwrap().digest_hex(),
        declared.digest_hex(),
        "decode then encode must be the identity on a valid reference"
    );

    let valid = declared.digest_hex().to_string();
    let refuse = |what: &str, hex: &str| {
        let fingerprint = AudioFingerprint::new(apw_trace::fingerprint::ALGORITHM_ID, hex).unwrap();
        assert!(
            ReferenceFingerprint::from_manifest_fingerprint(&fingerprint).is_err(),
            "{what} was accepted"
        );
    };
    refuse("a blob with the wrong magic", &format!("00{}", &valid[2..]));
    refuse("a truncated blob", &valid[..valid.len() - 8]);
    refuse("a blob with trailing bytes", &format!("{valid}00"));
    // Byte 5 onward is the varint frame count; zeroing the declared count leaves every peak past
    // the end of a zero-length recording, which the frame bound must catch.
    refuse(
        "a peak list that runs past the declared frame count",
        &format!("{}0101{}", &valid[..10], &valid[14..]),
    );

    // An unreadable ALGORITHM is not a corrupt blob: it is a descriptor this build cannot compare,
    // and it must report absence rather than an error or, worse, an affirmation.
    let other = AudioFingerprint::new("chromaprint-v1", &valid).unwrap();
    assert!(
        ReferenceFingerprint::from_manifest_fingerprint(&other)
            .unwrap()
            .is_none()
    );
}
