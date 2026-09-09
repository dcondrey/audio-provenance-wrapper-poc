use std::path::PathBuf;

use audio_provenance_core::CodedError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NeuralWatermarkError {
    #[error("model card `{path}` could not be read: {reason}")]
    CardUnreadable { path: PathBuf, reason: String },
    #[error("model card `{path}` is not well-formed: {reason}")]
    CardMalformed { path: PathBuf, reason: String },
    #[error("model card declares format `{found}`; this build reads `{expected}`")]
    CardFormat {
        found: String,
        expected: &'static str,
    },
    #[error("model card declares algorithm `{found}`; this crate hosts `{expected}`")]
    AlgorithmMismatch {
        found: String,
        expected: &'static str,
    },
    #[error("graph file `{path}` could not be read: {reason}")]
    GraphUnreadable { path: PathBuf, reason: String },
    /// IMPORTANT: the model file is trusted code-equivalent. A swapped model mints accepts, so a
    /// digest mismatch is refused rather than warned about (spec 7.5).
    #[error("graph `{graph}` hashes to {found}; its card pins {expected}")]
    GraphHashMismatch {
        graph: &'static str,
        expected: String,
        found: String,
    },
    #[error("model card sets transform field `{field}` to {found}; this build requires {expected}")]
    TransformMismatch {
        field: &'static str,
        expected: String,
        found: String,
    },
    #[error("model card names graph file `{file}`, which leaves the card's own directory")]
    GraphPathRejected { file: String },
    #[error("graph `{graph}` declares no tensor named `{name}`")]
    GraphIoMissing { graph: &'static str, name: String },
    #[error("graph `{graph}` tensor `{name}` has shape {found}; the card's contract is {expected}")]
    GraphShapeMismatch {
        graph: &'static str,
        name: String,
        expected: String,
        found: String,
    },
    #[error("ONNX Runtime refused to build the `{graph}` session: {reason}")]
    SessionBuild { graph: &'static str, reason: String },
    #[error("ONNX Runtime failed running the `{graph}` graph: {reason}")]
    Inference { graph: &'static str, reason: String },
    #[error("the `{graph}` session lock was poisoned by a panic on another thread")]
    SessionPoisoned { graph: &'static str },
    #[error("this model card carries no encoder graph, so embedding is unavailable")]
    EncoderUnavailable,
    #[error("model card threshold `{field}` holds {found}, outside its {range} range")]
    ThresholdRange {
        field: &'static str,
        found: f64,
        range: &'static str,
    },
    /// A fixture is a plumbing artifact. Letting one carry a calibration record would let a
    /// hand-written arithmetic graph report a measured acoustic capability.
    #[error("model card is marked `fixture` and also carries an operating envelope")]
    FixtureCarriesEnvelope,
    #[error("operating envelope rejected: {reason}")]
    EnvelopeRejected { reason: String },
    #[error("sample rate {found} is below the {min} Hz floor the mark is defined at")]
    SampleRateTooLow { found: u32, min: u32 },
    #[error("sample rate {found} is above the {max} Hz ceiling this build accepts")]
    SampleRateTooHigh { found: u32, max: u32 },
    #[error("audio holds no frames")]
    Empty,
    #[error("audio runs {found_s:.3} s, under the {needed_s:.3} s this tier reads")]
    TooShort { found_s: f64, needed_s: f64 },
    #[error("sample at channel {channel}, frame {frame} is not finite")]
    NonFinite { channel: usize, frame: usize },
    #[error("payload is {found} bytes; the Watermark-N payload is exactly {expected}")]
    PayloadLength { found: usize, expected: usize },
    #[error("payload declares version {found}; this build writes and accepts only {expected}")]
    PayloadVersion { found: u8, expected: u8 },
    #[error("payload field `{field}` holds {found}, over the {max} its {bits}-bit width allows")]
    PayloadFieldRange {
        field: &'static str,
        found: u64,
        max: u64,
        bits: u32,
    },
    #[error(transparent)]
    Audio(#[from] audio_provenance_audio::AudioError),
}

impl CodedError for NeuralWatermarkError {
    fn code(&self) -> &'static str {
        match self {
            Self::CardUnreadable { .. } => "model_card_unreadable",
            Self::CardMalformed { .. } => "model_card_malformed",
            Self::CardFormat { .. } => "model_card_format_unknown",
            Self::AlgorithmMismatch { .. } => "model_algorithm_mismatch",
            Self::GraphUnreadable { .. } => "model_graph_unreadable",
            Self::GraphHashMismatch { .. } => "model_weights_hash_mismatch",
            Self::TransformMismatch { .. } => "model_transform_mismatch",
            Self::GraphPathRejected { .. } => "model_graph_path_rejected",
            Self::GraphIoMissing { .. } => "model_graph_io_missing",
            Self::GraphShapeMismatch { .. } => "model_graph_shape_mismatch",
            Self::SessionBuild { .. } => "onnx_session_failed",
            Self::Inference { .. } => "onnx_inference_failed",
            Self::SessionPoisoned { .. } => "onnx_session_poisoned",
            Self::EncoderUnavailable => "model_unavailable",
            Self::ThresholdRange { .. } => "model_threshold_out_of_range",
            Self::FixtureCarriesEnvelope => "fixture_card_carries_envelope",
            Self::EnvelopeRejected { .. } => "acoustic_envelope_rejected",
            Self::SampleRateTooLow { .. } => "fs_too_low",
            Self::SampleRateTooHigh { .. } => "fs_too_high",
            Self::Empty => "audio_empty",
            Self::TooShort { .. } => "audio_too_short",
            Self::NonFinite { .. } => "audio_non_finite_sample",
            Self::PayloadLength { .. } => "payload_length_invalid",
            Self::PayloadVersion { .. } => "payload_version_unsupported",
            Self::PayloadFieldRange { .. } => "payload_field_out_of_range",
            Self::Audio(inner) => inner.code(),
        }
    }
}
