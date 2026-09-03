//! Offset-histogram scoring, re-derived locally from postings.
//!
//! # `coherence_ratio` is not the POC's `confidence`
//!
//! `daemon/audio_association.py` computes a weighted point similarity over relative RMS, ZCR, crest
//! and a four-band energy envelope, accepted at `>= 0.74`. That number describes feature alignment
//! between two recordings the caller already believes are related. This one describes how much of a
//! query's landmark evidence lands on one time offset in one indexed recording. They are different
//! quantities on different inputs; never compare them and never reuse the threshold.

use std::collections::{HashMap, HashSet};

use audio_provenance_registry::RecordId;

use crate::error::GenoTraceError;
use crate::fingerprint::index::{DEFAULT_POSTING_BUDGET, FingerprintIndex};
use crate::fingerprint::landmark::{FRAMES_PER_SECOND, Landmark};

/// Aligned anchors in the peak bin must total at least this many query hits.
pub const MIN_PEAK: u32 = 15;
/// The peak must beat the largest bin outside its neighbourhood by this factor.
pub const PEAK_DOMINANCE: f64 = 3.0;
/// Frames either side of the peak that count as "inside" it.
pub const PEAK_NEIGHBOURHOOD_FRAMES: i64 = 5;
/// Aligned anchors must span at least this much query time.
pub const MIN_ALIGNED_SPAN_SECONDS: f64 = 3.0;
/// Below this the query carries too little evidence for any answer.
pub const MIN_QUERY_SECONDS: f64 = 5.0;

/// One track's alignment evidence.
#[derive(Debug, Clone)]
pub struct TrackAlignment {
    pub track: u64,
    pub record: RecordId,
    pub peak: u32,
    pub peak_offset_frames: i64,
    pub best_outside_peak: u32,
    pub matched_query_hashes: usize,
    pub aligned_span_seconds: f64,
    pub coherence_ratio: f64,
    /// Query anchors that landed on the winning offset. Kept inside the
    /// crate so reference corroboration can measure local coverage instead of
    /// inferring coverage from the first and last hit.
    pub(crate) aligned_query_frames: Vec<u32>,
    /// Locally coherent regions whose offset disagrees with the global one.
    /// Reordered or duplicated sections create these even when one global
    /// offset remains dominant.
    pub local_offset_discontinuities: usize,
}

impl TrackAlignment {
    /// Every gate the spec names, all of which must hold.
    pub fn is_hit(&self, query_seconds: f64) -> bool {
        query_seconds >= MIN_QUERY_SECONDS
            && self.peak >= MIN_PEAK
            && f64::from(self.peak) >= PEAK_DOMINANCE * f64::from(self.best_outside_peak)
            && self.aligned_span_seconds >= MIN_ALIGNED_SPAN_SECONDS
    }
}

#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub best: Option<TrackAlignment>,
    pub runner_up: Option<TrackAlignment>,
    pub postings_scanned: usize,
    pub query_hashes: usize,
    /// True when the posting budget stopped the scan. The rung aborts rather than reporting the
    /// partial result, because a truncated scan understates every score it did not finish.
    pub budget_exceeded: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct ScoreLimits {
    pub posting_budget: usize,
}

impl Default for ScoreLimits {
    fn default() -> Self {
        Self {
            posting_budget: DEFAULT_POSTING_BUDGET,
        }
    }
}

#[derive(Default)]
struct Accumulator {
    /// Offsets `t1_ref - t1_query`, with the query anchor time kept so the aligned span is
    /// measurable rather than assumed.
    offsets: Vec<(i64, u32)>,
    matched_hashes: usize,
}

/// Scans an index for the query's landmarks and re-derives every score locally.
pub fn search(
    index: &dyn FingerprintIndex,
    query: &[Landmark],
    limits: ScoreLimits,
) -> Result<SearchOutcome, GenoTraceError> {
    let mut tracks: HashMap<u64, Accumulator> = HashMap::new();
    let mut scanned = 0usize;
    let mut budget_exceeded = false;
    let mut seen_tracks: HashSet<u64> = HashSet::new();

    // Distinct hashes only: the query emits each pair three times with a smeared target bin, and
    // counting one landmark three times would inflate every peak it lands in.
    let mut distinct: Vec<Landmark> = query.to_vec();
    distinct.sort_unstable_by_key(|landmark| (landmark.hash, landmark.t1));
    distinct.dedup_by_key(|landmark| (landmark.hash, landmark.t1));

    'scan: for landmark in &distinct {
        let postings = index.postings_for(landmark.hash)?;
        if postings.is_empty() {
            continue;
        }
        seen_tracks.clear();
        for posting in postings {
            scanned += 1;
            if scanned > limits.posting_budget {
                budget_exceeded = true;
                break 'scan;
            }
            let offset = i64::from(posting.t1) - i64::from(landmark.t1);
            let entry = tracks.entry(posting.track).or_default();
            entry.offsets.push((offset, landmark.t1));
            if seen_tracks.insert(posting.track) {
                entry.matched_hashes += 1;
            }
        }
    }

    if budget_exceeded {
        return Ok(SearchOutcome {
            best: None,
            runner_up: None,
            postings_scanned: scanned,
            query_hashes: distinct.len(),
            budget_exceeded: true,
        });
    }

    let mut alignments: Vec<TrackAlignment> = Vec::new();
    for (track, accumulator) in tracks {
        let Some(record) = index.record_id(track) else {
            return Err(GenoTraceError::FingerprintIndexMalformed {
                reason: format!("track {track} has postings but no record id"),
            });
        };
        if let Some(alignment) = align(track, record, &accumulator) {
            alignments.push(alignment);
        }
    }
    alignments.sort_by(|left, right| {
        right
            .peak
            .cmp(&left.peak)
            .then_with(|| right.coherence_ratio.total_cmp(&left.coherence_ratio))
            .then_with(|| left.track.cmp(&right.track))
    });

    let mut iterator = alignments.into_iter();
    let best = iterator.next();
    let runner_up = iterator.next();

    Ok(SearchOutcome {
        best,
        runner_up,
        postings_scanned: scanned,
        query_hashes: distinct.len(),
        budget_exceeded: false,
    })
}

/// Histogram into one-frame bins, smoothed by adding both neighbours; the peak is the largest
/// smoothed bin, and a diffuse pileup is rejected by comparing it against the best bin outside its
/// neighbourhood.
fn align(track: u64, record: RecordId, accumulator: &Accumulator) -> Option<TrackAlignment> {
    if accumulator.offsets.is_empty() {
        return None;
    }
    let mut histogram: HashMap<i64, u32> = HashMap::new();
    for (offset, _) in &accumulator.offsets {
        *histogram.entry(*offset).or_insert(0) += 1;
    }

    let mut smoothed: Vec<(i64, u32)> = histogram
        .keys()
        .map(|offset| {
            let total = histogram.get(&(offset - 1)).copied().unwrap_or(0)
                + histogram.get(offset).copied().unwrap_or(0)
                + histogram.get(&(offset + 1)).copied().unwrap_or(0);
            (*offset, total)
        })
        .collect();
    smoothed.sort_unstable();

    let (peak_offset, peak) = smoothed
        .iter()
        .copied()
        .max_by_key(|(offset, total)| (*total, std::cmp::Reverse(*offset)))?;

    let best_outside_peak = smoothed
        .iter()
        .filter(|(offset, _)| (offset - peak_offset).abs() > PEAK_NEIGHBOURHOOD_FRAMES)
        .map(|(_, total)| *total)
        .max()
        .unwrap_or(0);

    // The span is measured over the query anchors that actually landed in the peak bin, so a single
    // transient coincidence cannot present as a long alignment.
    let mut aligned: Vec<u32> = accumulator
        .offsets
        .iter()
        .filter(|(offset, _)| (offset - peak_offset).abs() <= 1)
        .map(|(_, query_t1)| *query_t1)
        .collect();
    aligned.sort_unstable();
    aligned.dedup();
    let span_frames = match (aligned.iter().min(), aligned.iter().max()) {
        (Some(first), Some(last)) => u64::from(last - first),
        _ => 0,
    };
    let aligned_span_seconds = span_frames as f64 / FRAMES_PER_SECOND;

    let coherence_ratio = if accumulator.matched_hashes == 0 {
        0.0
    } else {
        (f64::from(peak) / accumulator.matched_hashes as f64).clamp(0.0, 1.0)
    };

    // One-second local offset estimates. Only a locally dominant peak with at
    // least five anchors counts, so random hash collisions cannot manufacture
    // a discontinuity. A real reordered section remains coherent locally but
    // lands far from the global offset.
    const LOCAL_REGION_FRAMES: u32 = FRAMES_PER_SECOND as u32;
    const MIN_LOCAL_PEAK: u32 = 5;
    let mut local_histograms: HashMap<u32, HashMap<i64, u32>> = HashMap::new();
    for (offset, query_t1) in &accumulator.offsets {
        let region = *query_t1 / LOCAL_REGION_FRAMES.max(1);
        *local_histograms
            .entry(region)
            .or_default()
            .entry(*offset)
            .or_insert(0) += 1;
    }
    let local_offset_discontinuities = local_histograms
        .values()
        .filter(|histogram| {
            let Some((&local_offset, &local_peak)) = histogram
                .iter()
                .max_by_key(|(offset, count)| (**count, std::cmp::Reverse(**offset)))
            else {
                return false;
            };
            if local_peak < MIN_LOCAL_PEAK
                || (local_offset - peak_offset).abs() <= PEAK_NEIGHBOURHOOD_FRAMES
            {
                return false;
            }
            let outside = histogram
                .iter()
                .filter(|(offset, _)| (**offset - local_offset).abs() > 1)
                .map(|(_, count)| *count)
                .max()
                .unwrap_or(0);
            f64::from(local_peak) >= PEAK_DOMINANCE * f64::from(outside)
        })
        .count();

    Some(TrackAlignment {
        track,
        record,
        peak,
        peak_offset_frames: peak_offset,
        best_outside_peak,
        matched_query_hashes: accumulator.matched_hashes,
        aligned_span_seconds,
        coherence_ratio,
        aligned_query_frames: aligned,
        local_offset_discontinuities,
    })
}

#[cfg(test)]
mod discontinuity_tests {
    use super::*;

    #[test]
    fn a_locally_coherent_reordered_region_is_not_hidden_by_the_global_peak() {
        let mut offsets = Vec::new();
        let region_frames = FRAMES_PER_SECOND as u32;
        for region in 0..10u32 {
            let offset = if region == 4 { 120 } else { 0 };
            for anchor in 0..8u32 {
                offsets.push((offset, region * region_frames + anchor * 3));
            }
        }
        let accumulator = Accumulator {
            matched_hashes: offsets.len(),
            offsets,
        };
        let record = RecordId::parse_hex(&"ab".repeat(32)).unwrap();
        let alignment = align(7, record, &accumulator).unwrap();

        assert_eq!(alignment.peak_offset_frames, 0);
        assert_eq!(alignment.local_offset_discontinuities, 1);
        assert!(alignment.is_hit(10.0));
    }
}
