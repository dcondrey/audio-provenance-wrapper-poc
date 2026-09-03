//! The routed/export audio association: bounded feature extraction and the
//! gain-normalised offset search that decides whether an export plausibly
//! contains the audio a session routed through the plug-in.
//!
//! IMPORTANT: an established match is an INFERENCE from four bounded features,
//! never an observation of the export's own production. The record it produces
//! carries `ProofLevel::Inferred` at best, derived from the status so it cannot
//! be raised, and a failed or unavailable comparison is never evidence that the
//! routed audio is absent from the export.
//!
//! `daemon/audio_association.py` is the behavioural oracle; the arithmetic,
//! thresholds, iteration order and rendered rounding here reproduce it.

mod associate;
mod error;
mod extract;
mod feature;
mod pyround;
mod record;
mod similarity;
mod wav;

pub use associate::associate_export;
pub use error::{AssocError, Result};
pub use extract::{extract_feature_sequence, extract_from_source, open_pcm, MIN_WINDOW_FRAMES};
pub use feature::{decode_pcm, ByteOrder, ExtractionDetails, Feature};
pub use pyround::{python_round, python_round_to_i64};
pub use record::{
    Alignment, AlignmentPoint, AssociationRecord, COMPARISON_LIMITATIONS, FEATURE_DIMENSIONS,
    MAX_FEATURE_WINDOWS, METHOD, METHOD_VERSION, NOT_ESTABLISHED_REASON, ROUTED_FEATURE_SOURCE,
    UNAVAILABLE_LIMITATIONS,
};
pub use similarity::{
    alignment_offsets, compare_feature_sequences, gain_reference, point_similarity,
    ALIGNMENT_CHART_POINTS, ALIGNMENT_THRESHOLD, CREST_TOLERANCE_OCTAVES, CREST_WEIGHT,
    ENVELOPE_TOLERANCE, ENVELOPE_WEIGHT, MAX_ALIGNMENT_OFFSETS, MAX_COMPARISON_POINTS,
    MIN_COMPARABLE_WINDOWS, MIN_CONFIDENCE, MIN_MATCHED_COVERAGE, MIN_ROUTED_COVERAGE, RMS_TOLERANCE_DB,
    RMS_WEIGHT, SILENCE_RMS, ZCR_TOLERANCE, ZCR_WEIGHT,
};
pub use wav::{PcmSource, WavSource};
