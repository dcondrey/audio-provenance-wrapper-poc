//! What this build can actually do, in a shape a caller can print or serialise.

use serde::Serialize;

/// The literal string `"unsupported"`, never `null` and never absent.
///
/// `null` reads as "not yet measured" and invites hope. No published method demonstrates blind,
/// CRC-gated, ground-truth-free recovery over a speaker-to-microphone path at any distance.
pub const ACOUSTIC_RERECORDING: &str = apw_watermark::ACOUSTIC_RERECORDING;

/// The rate every figure in [`capabilities`] is quoted at.
pub const REFERENCE_SAMPLE_RATE: u32 = 44_100;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Capabilities {
    pub sdk: &'static str,
    pub version: &'static str,
    pub sample_rate_hz: u32,
    pub canonicalization: &'static str,
    pub manifest_schemas: [&'static str; 2],
    pub signature_algorithms: [&'static str; 2],
    pub recovery_methods: [&'static str; 6],
    pub registry_backends: [&'static str; 2],
    pub decodes: [&'static str; 6],
    pub encodes: [&'static str; 4],
    pub mark: MarkCapabilities,
    pub fingerprint: FingerprintCapabilities,
    /// Repeated at the top level so a caller that reads nothing else still reads this one.
    pub acoustic_rerecording: &'static str,
    pub default_soft_binding_threshold: f64,
    /// A soft binding cannot reach `verified` without a measured false-positive rate, and the only
    /// source of one is a `audio-provenance-bench` null-test report the caller supplies.
    pub soft_binding_verified_requires_null_test: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MarkCapabilities {
    pub algorithm: &'static str,
    pub payload_bits: usize,
    pub sample_rate_hz: (u32, u32),
    pub band_hz: (f64, f64),
    pub block_seconds: f64,
    pub guaranteed_decode_seconds: f64,
    pub strong_class_seconds: f64,
    /// Playback-rate deviation the detector searches.
    pub searched_rate_range: f64,
    /// Playback-rate deviation recovery was measured to survive; smaller than the searched range on
    /// purpose, because the grid is allowed a margin and the claim is not.
    pub measured_rate_range: f64,
    /// `None` until a PEAQ campaign and a blinded ABX panel have run. The SDK never claims
    /// inaudibility it has not measured.
    pub measured_transparency: Option<f64>,
    /// `None` until a robustness campaign against the product corpus has run.
    pub measured_survival: Option<f64>,
    pub acoustic_rerecording: &'static str,
    pub time_stretch: &'static str,
    /// Namespace 0's profile key is published, so the public mark is removable by an informed
    /// adversary. It is provenance recovery, not tamper resistance.
    pub public_namespace_is_removable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FingerprintCapabilities {
    pub algorithm: &'static str,
    pub min_query_seconds: f64,
    /// Always false, and it is about RUNG 5, the search. A fingerprint SEARCH recovers no binding
    /// from the audio; it guesses a record from perceptual similarity, so it is `inferred` and the
    /// status mapping has no arm taking it to `verified`.
    ///
    /// Do not read it as "a fingerprint plays no part in any `verified`". See
    /// [`Self::signed_reference_corroborates_lossy_path`], which is a different thing on a
    /// different input: one record already in hand, compared against the constellation that record
    /// itself signed.
    pub can_reach_verified: bool,
    pub default_policy_emits_candidate: bool,
    /// A Audio Provenance record commits to the constellation of the audio it was signed over, so a
    /// transcode whose hard binding fails can still reach `verified` at proof level `inferred` when
    /// a CRC-valid Watermark payload for that record also decodes out of the audio.
    pub signed_reference_corroborates_lossy_path: bool,
    /// The alignment-coverage floor that corroboration must clear, on top of every rung-5 alignment
    /// gate and the block-coverage guard.
    pub reference_affirmation_coverage: f64,
}

/// What this build can do at 44.1 kHz.
pub fn capabilities() -> Capabilities {
    Capabilities::at(REFERENCE_SAMPLE_RATE)
}

impl Capabilities {
    pub fn at(sample_rate: u32) -> Self {
        let mark = apw_watermark::Capabilities::at(sample_rate);
        Self {
            sdk: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
            sample_rate_hz: sample_rate,
            canonicalization: audio_provenance_core::CANONICALIZATION_ID,
            manifest_schemas: [
                audio_provenance_manifest::APW_SCHEMA_ID,
                audio_provenance_manifest::AUDIO_PROVENANCE_SCHEMA_ID,
            ],
            signature_algorithms: ["ed25519", "ed25519-sha256-remote-custody"],
            recovery_methods: [
                apw_trace::RecoveryMethod::EmbeddedManifest.as_str(),
                apw_trace::RecoveryMethod::SidecarManifest.as_str(),
                apw_trace::RecoveryMethod::ContentHashLookup.as_str(),
                apw_trace::RecoveryMethod::DecodedAudioHashLookup.as_str(),
                apw_trace::RecoveryMethod::WatermarkRecovery.as_str(),
                apw_trace::RecoveryMethod::FingerprintSearch.as_str(),
            ],
            registry_backends: ["local", "http"],
            // IMPORTANT: `ogg_vorbis`, not `ogg`. symphonia 0.6 ships no Opus decoder and
            // `all-codecs` does not include one, so Ogg/Opus - today the common Ogg payload -
            // returns `input_undecodable`. Advertising the container would promise the codec.
            decodes: ["wav", "aiff", "mp3", "flac", "ogg_vorbis", "mp4"],
            encodes: ["wav_pcm_16", "wav_pcm_24", "wav_pcm_32", "wav_float32"],
            mark: MarkCapabilities {
                algorithm: mark.algorithm,
                payload_bits: mark.payload_bits,
                sample_rate_hz: mark.sample_rate_range,
                band_hz: mark.band_hz,
                block_seconds: mark.block_seconds,
                guaranteed_decode_seconds: mark.guaranteed_seconds,
                strong_class_seconds: mark.strong_class_seconds,
                searched_rate_range: mark.searched_rate_range,
                measured_rate_range: mark.measured_rate_range,
                measured_transparency: mark.measured_transparency,
                measured_survival: mark.measured_survival,
                acoustic_rerecording: mark.acoustic_rerecording,
                time_stretch: mark.time_stretch,
                public_namespace_is_removable: true,
            },
            fingerprint: FingerprintCapabilities {
                algorithm: apw_trace::fingerprint::ALGORITHM_ID,
                min_query_seconds: apw_trace::fingerprint::MIN_QUERY_SECONDS,
                can_reach_verified: false,
                default_policy_emits_candidate: false,
                signed_reference_corroborates_lossy_path: true,
                reference_affirmation_coverage: apw_trace::AFFIRMATION_COVERAGE,
            },
            acoustic_rerecording: ACOUSTIC_RERECORDING,
            default_soft_binding_threshold: apw_trace::DEFAULT_SOFT_BINDING_THRESHOLD,
            soft_binding_verified_requires_null_test: true,
        }
    }
}
