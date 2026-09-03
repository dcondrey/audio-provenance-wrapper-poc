//! Rung 5's fingerprint: landmark extraction, the posting index, and local score re-derivation.
//!
//! Nothing here recovers a binding from the audio. It guesses a manifest from perceptual
//! similarity, which is why the whole rung is `inferred` and why it can never produce `verified`.

pub mod index;
pub mod landmark;
pub mod reference;
pub mod score;

use audio_provenance_audio::AudioBuffer;
use audio_provenance_registry::RecordId;

use crate::error::GenoTraceError;
use crate::fingerprint::index::FingerprintIndex;
use crate::fingerprint::landmark::{Constellation, Landmark, RATE_SWEEP};
use crate::fingerprint::score::{ScoreLimits, TrackAlignment};

pub use index::{
    DEFAULT_POSTING_BUDGET, FileFingerprintIndex, MAX_POSTINGS_PER_HASH, MemoryFingerprintIndex,
    Posting,
};
pub use landmark::{ALGORITHM_ID, SCHEME_VERSION};
pub use reference::{
    AFFIRMATION_COVERAGE, Affirmation, ReferenceFingerprint, compare, reference_fingerprint,
};
pub use score::{MIN_QUERY_SECONDS, SearchOutcome};

/// A query's constellation, kept so the rate sweep can reuse it instead of re-running the STFT.
#[derive(Debug, Clone)]
pub struct FingerprintQuery {
    constellation: Constellation,
    seconds: f64,
}

impl FingerprintQuery {
    pub fn from_audio(audio: &AudioBuffer) -> Result<Self, GenoTraceError> {
        let samples = landmark::preprocess(audio)?;
        let constellation = landmark::constellation(&samples)?;
        let seconds = constellation.duration_seconds();
        Ok(Self {
            constellation,
            seconds,
        })
    }

    pub const fn seconds(&self) -> f64 {
        self.seconds
    }

    pub fn peak_count(&self) -> usize {
        self.constellation.peaks().len()
    }

    pub fn is_long_enough(&self) -> bool {
        self.seconds >= MIN_QUERY_SECONDS
    }
}

/// Landmarks for the index side. No query smear and no rate sweep: the reference is what the query
/// is being compared against, so widening it would only blur the index.
pub fn reference_landmarks(audio: &AudioBuffer) -> Result<Vec<Landmark>, GenoTraceError> {
    let samples = landmark::preprocess(audio)?;
    let constellation = landmark::constellation(&samples)?;
    Ok(landmark::landmarks(&constellation))
}

#[derive(Debug, Clone)]
pub struct FingerprintHit {
    pub record: RecordId,
    pub alignment: TrackAlignment,
    /// The playback-rate hypothesis that produced the hit. `1.0` for the common case.
    pub rate: f64,
}

#[derive(Debug, Clone)]
pub struct FingerprintSearch {
    pub hit: Option<FingerprintHit>,
    /// The strongest alignment any rate hypothesis produced, accepted or not. A refusal reports the
    /// number it refused rather than a bare no.
    pub best_seen: Option<TrackAlignment>,
    /// Two tracks both cleared every gate, so no identity can be reported from this rung.
    pub ambiguous: bool,
    pub budget_exceeded: bool,
    pub postings_scanned: usize,
    pub query_hashes: usize,
    pub rates_tried: usize,
    pub too_short: bool,
}

/// Runs the query against the index at rate 1.00, and only then sweeps the rate grid.
///
/// The sweep exists for a playback-rate change, which moves both axes at once; the common case
/// pays nothing for it. The whole search shares one posting budget, so a sweep cannot buy five
/// times the scan.
pub fn search(
    index: &dyn FingerprintIndex,
    query: &FingerprintQuery,
    limits: ScoreLimits,
) -> Result<FingerprintSearch, GenoTraceError> {
    if !query.is_long_enough() {
        return Ok(FingerprintSearch {
            hit: None,
            best_seen: None,
            ambiguous: false,
            budget_exceeded: false,
            postings_scanned: 0,
            query_hashes: 0,
            rates_tried: 0,
            too_short: true,
        });
    }

    let mut scanned = 0usize;
    let mut query_hashes = 0usize;
    let mut rates_tried = 0usize;
    let mut remaining = limits.posting_budget;
    let mut best_seen: Option<TrackAlignment> = None;

    for rate in std::iter::once(1.0).chain(RATE_SWEEP) {
        let constellation = if rate == 1.0 {
            query.constellation.clone()
        } else {
            landmark::rescale(&query.constellation, rate)
        };
        let hashes = landmark::query_landmarks(&constellation);
        let outcome = score::search(
            index,
            &hashes,
            ScoreLimits {
                posting_budget: remaining,
            },
        )?;
        rates_tried += 1;
        scanned += outcome.postings_scanned;
        query_hashes = query_hashes.max(outcome.query_hashes);
        remaining = remaining.saturating_sub(outcome.postings_scanned);

        if outcome.budget_exceeded {
            return Ok(FingerprintSearch {
                hit: None,
                best_seen,
                ambiguous: false,
                budget_exceeded: true,
                postings_scanned: scanned,
                query_hashes,
                rates_tried,
                too_short: false,
            });
        }

        if let Some(candidate) = &outcome.best
            && best_seen
                .as_ref()
                .is_none_or(|held| candidate.peak > held.peak)
        {
            best_seen = Some(candidate.clone());
        }

        let Some(best) = outcome.best.filter(|best| best.is_hit(query.seconds)) else {
            continue;
        };
        let ambiguous = outcome
            .runner_up
            .is_some_and(|runner_up| runner_up.is_hit(query.seconds));
        return Ok(FingerprintSearch {
            hit: Some(FingerprintHit {
                record: best.record,
                alignment: best.clone(),
                rate,
            }),
            best_seen: Some(best),
            ambiguous,
            budget_exceeded: false,
            postings_scanned: scanned,
            query_hashes,
            rates_tried,
            too_short: false,
        });
    }

    Ok(FingerprintSearch {
        hit: None,
        best_seen,
        ambiguous: false,
        budget_exceeded: false,
        postings_scanned: scanned,
        query_hashes,
        rates_tried,
        too_short: false,
    })
}
