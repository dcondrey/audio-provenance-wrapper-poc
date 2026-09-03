use crate::feature::Feature;
use crate::pyround::python_round_to_i64;
use crate::record::{Alignment, AlignmentPoint, AssociationRecord};

/// A window whose RMS is at or above this is scored on its gain-normalised
/// level; both sides below it are scored as agreeing silence.
pub const SILENCE_RMS: f64 = 1e-7;
/// The relative-level difference, in dB, at which the RMS axis scores zero.
pub const RMS_TOLERANCE_DB: f64 = 15.0;
/// The zero-crossing-rate difference at which the ZCR axis scores zero.
pub const ZCR_TOLERANCE: f64 = 0.08;
/// The crest-factor difference, in octaves, at which the crest axis scores zero.
pub const CREST_TOLERANCE_OCTAVES: f64 = 1.5;
/// The mean per-band envelope difference at which the envelope axis scores zero.
pub const ENVELOPE_TOLERANCE: f64 = 0.75;

pub const RMS_WEIGHT: f64 = 0.25;
pub const ZCR_WEIGHT: f64 = 0.40;
pub const CREST_WEIGHT: f64 = 0.10;
pub const ENVELOPE_WEIGHT: f64 = 0.25;

/// A per-window similarity at or above this counts as a matched window.
pub const ALIGNMENT_THRESHOLD: f64 = 0.72;
pub const MIN_CONFIDENCE: f64 = 0.74;
pub const MIN_MATCHED_COVERAGE: f64 = 0.60;
/// An export must overlap at least this fraction of all routed windows for the
/// association to be established.
pub const MIN_ROUTED_COVERAGE: f64 = 0.25;
pub const MIN_COMPARABLE_WINDOWS: usize = 3;

pub const MAX_ALIGNMENT_OFFSETS: usize = 801;
pub const MAX_COMPARISON_POINTS: usize = 600;
pub const ALIGNMENT_CHART_POINTS: usize = 56;

/// The median level, in dBFS, of the non-silent windows of a sequence. Both
/// sides are compared relative to their own reference, which is what lets a
/// fixed gain change survive the comparison.
pub fn gain_reference(sequence: &[Feature]) -> f64 {
    let mut levels: Vec<f64> = sequence
        .iter()
        .filter_map(|feature| feature.rms())
        .filter(|rms| *rms > SILENCE_RMS)
        .map(|rms| 20.0 * rms.max(1e-9).log10())
        .collect();
    if levels.is_empty() {
        return -180.0;
    }
    levels.sort_by(f64::total_cmp);
    let count = levels.len();
    if count % 2 == 1 {
        levels.get(count / 2).copied().unwrap_or(-180.0)
    } else {
        let high = levels.get(count / 2).copied().unwrap_or(-180.0);
        let low = levels.get(count / 2 - 1).copied().unwrap_or(-180.0);
        (low + high) / 2.0
    }
}

/// The weighted similarity of two windows in [0, 1]. An axis absent on either
/// side is dropped from both the numerator and the divisor, so a routed window
/// carrying only RMS and ZCR is scored on those two axes alone.
pub fn point_similarity(
    left: &Feature,
    right: &Feature,
    left_gain_reference: f64,
    right_gain_reference: f64,
) -> f64 {
    let mut weighted: Vec<(f64, f64)> = Vec::with_capacity(4);

    if let (Some(left_rms), Some(right_rms)) = (left.rms(), right.rms()) {
        let left_silent = left_rms < SILENCE_RMS;
        let right_silent = right_rms < SILENCE_RMS;
        if left_silent != right_silent {
            weighted.push((RMS_WEIGHT, 0.0));
        } else if left_silent {
            weighted.push((RMS_WEIGHT, 1.0));
        } else {
            let left_relative_db = 20.0 * left_rms.log10() - left_gain_reference;
            let right_relative_db = 20.0 * right_rms.log10() - right_gain_reference;
            let distance = (left_relative_db - right_relative_db).abs();
            weighted.push((RMS_WEIGHT, (1.0 - distance / RMS_TOLERANCE_DB).max(0.0)));
        }
    }

    if let (Some(left_zcr), Some(right_zcr)) = (left.zcr(), right.zcr()) {
        let distance = (left_zcr - right_zcr).abs();
        weighted.push((ZCR_WEIGHT, (1.0 - distance / ZCR_TOLERANCE).max(0.0)));
    }

    if let (Some(left_crest), Some(right_crest)) = (left.crest(), right.crest()) {
        if left_crest > 0.0 && right_crest > 0.0 {
            let distance = (left_crest / right_crest).log2().abs();
            weighted.push((
                CREST_WEIGHT,
                (1.0 - distance / CREST_TOLERANCE_OCTAVES).max(0.0),
            ));
        }
    }

    if let (Some(left_envelope), Some(right_envelope)) = (&left.envelope, &right.envelope) {
        if left_envelope.len() == right_envelope.len() && !left_envelope.is_empty() {
            let total = left_envelope
                .iter()
                .zip(right_envelope.iter())
                .fold(0.0f64, |sum, (left_value, right_value)| {
                    sum + (left_value - right_value).abs()
                });
            let distance = total / left_envelope.len() as f64;
            weighted.push((
                ENVELOPE_WEIGHT,
                (1.0 - distance / ENVELOPE_TOLERANCE).max(0.0),
            ));
        }
    }

    if weighted.is_empty() {
        return 0.0;
    }
    let weight = weighted.iter().fold(0.0f64, |sum, (axis, _)| sum + axis);
    weighted
        .iter()
        .fold(0.0f64, |sum, (axis, score)| sum + axis * score)
        / weight
}

/// Candidate export-minus-routed window offsets, subsampled to a bounded count.
pub fn alignment_offsets(routed_count: usize, export_count: usize) -> Vec<i64> {
    let low = 1i64 - routed_count as i64;
    let high = export_count as i64 - 1;
    let count = high - low + 1;
    if count <= MAX_ALIGNMENT_OFFSETS as i64 {
        return (low..=high).collect();
    }
    let span = high - low;
    let divisor = (MAX_ALIGNMENT_OFFSETS - 1) as f64;
    let mut offsets = Vec::with_capacity(MAX_ALIGNMENT_OFFSETS);
    let mut previous: Option<i64> = None;
    for index in 0..MAX_ALIGNMENT_OFFSETS as i64 {
        let position = low as f64 + (index * span) as f64 / divisor;
        let Some(value) = python_round_to_i64(position) else {
            continue;
        };
        if previous != Some(value) {
            offsets.push(value);
            previous = Some(value);
        }
    }
    offsets
}

/// Offset-search comparison of a routed feature sequence against an export's.
///
/// The result is an inference from bounded features, never an observation of
/// the export's own bytes, so an established match is labelled `inferred`.
pub fn compare_feature_sequences(
    routed: &[Feature],
    exported: &[Feature],
    window_seconds: f64,
) -> AssociationRecord {
    if routed.is_empty() {
        return AssociationRecord::unavailable("no routed feature windows were received");
    }
    if exported.is_empty() {
        return AssociationRecord::unavailable(
            "no comparable export feature windows were extracted",
        );
    }

    let routed_gain = gain_reference(routed);
    let export_gain = gain_reference(exported);
    let mut best: Option<(f64, f64, i64, usize, Vec<f64>)> = None;

    for offset in alignment_offsets(routed.len(), exported.len()) {
        let routed_start = offset.min(0).unsigned_abs() as usize;
        let export_start = offset.max(0) as usize;
        let routed_window = routed.get(routed_start..).unwrap_or_default();
        let export_window = exported.get(export_start..).unwrap_or_default();
        let overlap = routed_window.len().min(export_window.len());
        if overlap < MIN_COMPARABLE_WINDOWS {
            continue;
        }
        let stride = ((overlap as f64 / MAX_COMPARISON_POINTS as f64).ceil() as usize).max(1);
        let scores: Vec<f64> = routed_window
            .iter()
            .zip(export_window.iter())
            .take(overlap)
            .step_by(stride)
            .map(|(left, right)| point_similarity(left, right, routed_gain, export_gain))
            .collect();
        if scores.is_empty() {
            continue;
        }
        let comparable = scores.len() as f64;
        let mean = scores.iter().fold(0.0f64, |sum, score| sum + score) / comparable;
        let matched = matched_count(&scores) as f64 / comparable;
        let routed_coverage = overlap as f64 / routed.len().max(1) as f64;
        let confidence = 0.85 * mean + 0.15 * matched;
        let objective = confidence * (0.70 + 0.30 * routed_coverage.min(1.0));
        if best.as_ref().is_none_or(|current| objective > current.0) {
            best = Some((objective, confidence, offset, overlap, scores));
        }
    }

    let Some((_objective, confidence, offset, overlap, scores)) = best else {
        return AssociationRecord::unavailable("feature sequences had no usable overlap");
    };

    let matched_window_count = matched_count(&scores);
    let comparable_window_count = scores.len();
    let matched_coverage = matched_window_count as f64 / comparable_window_count as f64;
    let routed_coverage = overlap as f64 / routed.len().max(1) as f64;
    let established = confidence >= MIN_CONFIDENCE
        && matched_coverage >= MIN_MATCHED_COVERAGE
        && routed_coverage >= MIN_ROUTED_COVERAGE
        && comparable_window_count >= MIN_COMPARABLE_WINDOWS;

    let chart_stride = ((comparable_window_count as f64 / ALIGNMENT_CHART_POINTS as f64).ceil()
        as usize)
        .max(1);
    let series: Vec<AlignmentPoint> = scores
        .iter()
        .enumerate()
        .step_by(chart_stride)
        .map(|(index, score)| AlignmentPoint {
            relative_window: index,
            similarity: *score,
            matched: *score >= ALIGNMENT_THRESHOLD,
        })
        .collect();

    AssociationRecord::compared(
        established,
        confidence,
        matched_coverage,
        routed_coverage,
        matched_window_count,
        comparable_window_count,
        Alignment {
            overlap_window_count: overlap,
            routed_window_count: routed.len(),
            export_window_count: exported.len(),
            best_offset_windows: offset,
            best_offset_seconds: offset as f64 * window_seconds,
            series,
        },
    )
}

fn matched_count(scores: &[f64]) -> usize {
    scores
        .iter()
        .filter(|score| **score >= ALIGNMENT_THRESHOLD)
        .count()
}
