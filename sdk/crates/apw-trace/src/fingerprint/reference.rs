//! The reference constellation a signer commits to, and the comparison that re-derives it.
//!
//! # Why the manifest carries the peaks and not the landmark hashes
//!
//! Landmarks are a pure function of the constellation, so storing them would be a second spelling
//! of the same fact at eight times the size. Storing the peaks lets the verifier rebuild the
//! reference side with [`landmark::landmarks`] and the query side with
//! [`landmark::query_landmarks`], which is the asymmetry the scheme depends on: the query smears
//! its target bin and the reference must not.
//!
//! # This is not rung 5
//!
//! Rung 5 searches an index of many recordings and GUESSES which manifest a file belongs to; it is
//! `inferred` and can never verify. This module answers a different question about ONE manifest the
//! ladder already recovered: does the audio presented carry the same constellation the signer
//! committed to. It corroborates a recovered record; it never selects one.

use audio_provenance_audio::AudioBuffer;
use audio_provenance_manifest::AudioFingerprint;
use audio_provenance_registry::RecordId;

use crate::error::TraceError;
use crate::fingerprint::index::MemoryFingerprintIndex;
use crate::fingerprint::landmark::{
    self, ALGORITHM_ID, BAND_HIGH_BIN, BAND_LOW_BIN, Constellation, Peak, SCHEME_VERSION,
};
use crate::fingerprint::score::ScoreLimits;
use crate::fingerprint::{FingerprintQuery, search};

/// The fraction of locally expected regions in the PRESENTED audio that must
/// contain winning-offset landmarks from the signed reference, on top of every
/// [`crate::fingerprint::score::TrackAlignment::is_hit`] gate.
///
/// It is relative to the query rather than absolute, which is what makes it the right term for the
/// threat model. A crop or a transcode of the signed work aligns across nearly the whole file it is
/// presented as, however short that file is. A lifted mark carried on mostly-unrelated audio
/// aligns, at best, across the fragment that was lifted. `is_hit`'s absolute three-second span gate
/// cannot tell those apart on a long file; this can.
///
/// IMPORTANT: fixed here, never plumbed to a CLI flag and never widened to make a case pass. The
/// qualification target is in `docs/TRACE_SPEC.md`; changing this number requires re-running
/// the full null and adversarial corpora. The historical span-predicate measurement does not price
/// this newer composite predicate.
pub const AFFIRMATION_COVERAGE: f64 = 0.80;

/// Local coverage is priced in half-second regions. At the reference
/// constellation's target density this gives about fourteen signed peaks per
/// region, enough evidence to distinguish absence from one missed landmark.
pub const AFFIRMATION_REGION_SECONDS: f64 = 0.5;

/// A contiguous unexplained run longer than this rejects association even if
/// the global aligned fraction remains high. It is fixed rather than exposed as
/// a caller option: widening it after looking at an attack invalidates the gate.
pub const MAX_UNEXPLAINED_GAP_SECONDS: f64 = 0.75;

/// Container duration and playback-rate correction are approximate at the
/// STFT edges. This is the complete allowance when mapping a presented crop
/// into the duration committed by the signed reference.
pub const DURATION_MAPPING_TOLERANCE_SECONDS: f64 = 0.25;

const MIN_REFERENCE_ANCHORS_PER_REGION: usize = 3;

const MAGIC: [u8; 4] = *b"GTFR";

/// A signer's reference is bounded so a manifest cannot be used as a payload channel. At the
/// scheme's 28 peaks per second this is a little over two hours of audio.
pub const MAX_REFERENCE_PEAKS: usize = 262_144;

/// The matching bound on the declared frame count: the same two hours at 86.13 frames per second.
pub const MAX_REFERENCE_FRAMES: usize = 640_000;

/// The constellation a signed record commits to.
#[derive(Debug, Clone)]
pub struct ReferenceFingerprint {
    constellation: Constellation,
}

impl ReferenceFingerprint {
    /// Extracts the reference from the audio a manifest's hard binding will cover.
    ///
    /// `None` when the work is longer than [`MAX_REFERENCE_PEAKS`] can describe. A signer is told;
    /// nothing is silently decimated, because a thinned reference would fail to corroborate a
    /// legitimate transcode and report it as `changed`.
    pub fn from_audio(audio: &AudioBuffer) -> Result<Option<Self>, TraceError> {
        let samples = landmark::preprocess(audio)?;
        let constellation = landmark::constellation(&samples)?;
        if constellation.peaks().len() > MAX_REFERENCE_PEAKS {
            return Ok(None);
        }
        Ok(Some(Self { constellation }))
    }

    pub fn peak_count(&self) -> usize {
        self.constellation.peaks().len()
    }

    pub fn duration_seconds(&self) -> f64 {
        self.constellation.duration_seconds()
    }

    /// The `fingerprint` block a [`audio_provenance_manifest::ManifestDraft`] signs.
    pub fn to_manifest_fingerprint(&self) -> Result<AudioFingerprint, TraceError> {
        AudioFingerprint::new(ALGORITHM_ID, &hex::encode(self.encode())).map_err(|error| {
            TraceError::InvalidOption {
                option: "fingerprint",
                reason: error.to_string(),
            }
        })
    }

    /// Reads a signed record's `fingerprint` block.
    ///
    /// `Ok(None)` for a record that declares a fingerprint under some other algorithm: an
    /// unrecognised descriptor is evidence this build cannot read, never evidence of a mismatch.
    pub fn from_manifest_fingerprint(
        fingerprint: &AudioFingerprint,
    ) -> Result<Option<Self>, TraceError> {
        if fingerprint.algorithm() != ALGORITHM_ID {
            return Ok(None);
        }
        let bytes = hex::decode(fingerprint.digest_hex()).map_err(|error| {
            TraceError::InvalidOption {
                option: "fingerprint",
                reason: error.to_string(),
            }
        })?;
        Self::decode(&bytes).map(Some)
    }

    fn encode(&self) -> Vec<u8> {
        let peaks = self.constellation.peaks();
        let mut out = Vec::with_capacity(16 + peaks.len() * 3);
        out.extend_from_slice(&MAGIC);
        out.push(SCHEME_VERSION);
        put_varint(&mut out, self.constellation.frames() as u64);
        put_varint(&mut out, peaks.len() as u64);

        let mut previous = Peak {
            frame: 0,
            bin: BAND_LOW_BIN as u16,
        };
        for (index, peak) in peaks.iter().enumerate() {
            let frame_delta = u64::from(peak.frame.saturating_sub(previous.frame));
            put_varint(&mut out, frame_delta);
            // Within one frame the bins ascend strictly, so the difference from the previous bin is
            // the smaller number; a new frame restarts from the band floor.
            let bin_code = if frame_delta == 0 && index > 0 {
                u64::from(peak.bin.saturating_sub(previous.bin))
            } else {
                u64::from(peak.bin) - BAND_LOW_BIN as u64
            };
            put_varint(&mut out, bin_code);
            previous = *peak;
        }
        out
    }

    /// Every field is bounds-checked against the scheme's own limits: this blob arrives inside a
    /// manifest, which is untrusted input until the moment its signature verifies and is still
    /// attacker-chosen after it.
    fn decode(bytes: &[u8]) -> Result<Self, TraceError> {
        let mut cursor = Cursor::new(bytes);
        if cursor.take(4)? != MAGIC {
            return Err(malformed(
                "magic does not identify a reference constellation",
            ));
        }
        let version = cursor.take(1)?[0];
        if version != SCHEME_VERSION {
            return Err(malformed(format!(
                "reference scheme version {version} is not readable by this build"
            )));
        }
        // Capped as well as the peak count: `duration_seconds` is public, and a declared frame
        // count no encoder could have produced would report a duration out of nothing.
        let frames = usize::try_from(cursor.varint()?)
            .ok()
            .filter(|frames| *frames <= MAX_REFERENCE_FRAMES)
            .ok_or_else(|| {
                malformed(format!(
                    "reference declares a frame count over the {MAX_REFERENCE_FRAMES} cap"
                ))
            })?;
        let count = usize::try_from(cursor.varint()?)
            .map_err(|_| malformed("peak count does not fit this platform"))?;
        if count > MAX_REFERENCE_PEAKS {
            return Err(malformed(format!(
                "reference declares {count} peaks, over the {MAX_REFERENCE_PEAKS} cap"
            )));
        }

        let mut peaks = Vec::with_capacity(count);
        let mut previous = Peak {
            frame: 0,
            bin: BAND_LOW_BIN as u16,
        };
        for index in 0..count {
            let frame_delta = cursor.varint()?;
            let bin_code = cursor.varint()?;
            let frame = u64::from(previous.frame)
                .checked_add(frame_delta)
                .and_then(|frame| u32::try_from(frame).ok())
                .ok_or_else(|| malformed("frame index overflows"))?;
            let bin = if frame_delta == 0 && index > 0 {
                u64::from(previous.bin).saturating_add(bin_code)
            } else {
                BAND_LOW_BIN as u64 + bin_code
            };
            if !(BAND_LOW_BIN as u64..BAND_HIGH_BIN as u64).contains(&bin) {
                return Err(malformed(format!(
                    "peak bin {bin} is outside the hashing band"
                )));
            }
            if frames > 0 && frame as usize >= frames {
                return Err(malformed("peak frame runs past the declared frame count"));
            }
            let peak = Peak {
                frame,
                bin: bin as u16,
            };
            // Strict ascent is what the encoder guarantees and what the delta coding assumes. A
            // blob that violates it would decode to a different constellation than any encoder
            // could have produced.
            if index > 0 && peak <= previous {
                return Err(malformed("peaks are not in strictly ascending order"));
            }
            peaks.push(peak);
            previous = peak;
        }
        if !cursor.is_empty() {
            return Err(malformed("reference carries trailing bytes"));
        }
        Ok(Self {
            constellation: Constellation::from_parts(peaks, frames),
        })
    }
}

/// The `fingerprint` block a signer commits to, extracted from the audio its hard binding covers.
///
/// THE ONE SITE. Every signing path calls this; two independent computations of a signed field is a
/// divergence waiting to happen. `None` means the work is over [`MAX_REFERENCE_PEAKS`] and carries
/// no reference, which the caller must report rather than swallow.
pub fn reference_fingerprint(
    audio: &AudioBuffer,
) -> Result<Option<AudioFingerprint>, TraceError> {
    match ReferenceFingerprint::from_audio(audio)? {
        Some(reference) => reference.to_manifest_fingerprint().map(Some),
        None => Ok(None),
    }
}

/// What comparing a candidate against a signed reference established.
#[derive(Debug, Clone)]
pub struct Affirmation {
    /// True only when every rung-5 alignment gate and every local recording-association gate held.
    pub affirmed: bool,
    /// Every rung-5 alignment gate held: enough aligned anchors, a dominant offset bin, and a long
    /// enough aligned span. Reported separately from [`Self::affirmed`] so a measurement can say
    /// which term did the rejecting.
    pub gates_passed: bool,
    /// The share of locally expected regions that carry a winning-offset
    /// alignment, in `[0, 1]`. This is the quantity
    /// [`AFFIRMATION_COVERAGE`] gates and the quantity `VerifyResult.match`
    /// reports.
    pub coverage: f64,
    pub regions_expected: usize,
    pub regions_covered: usize,
    pub unexplained_regions: usize,
    pub max_unexplained_gap_seconds: f64,
    pub offset_discontinuities: usize,
    /// Whether the presented interval, after its recovered offset and rate are
    /// applied, fits within the approximate duration signed into the reference.
    pub duration_consistent: bool,
    pub reference_duration_seconds: f64,
    pub query_duration_seconds: f64,
    /// Diagnostic only. `peak / matched_hashes` is degenerate on a near-miss, where one matched
    /// hash lands in one bin and the ratio reads 1.0, so it is reported and never gated on.
    pub coherence_ratio: f64,
    pub peak: u32,
    pub best_outside_peak: u32,
    pub aligned_span_seconds: f64,
    /// The playback-rate hypothesis that produced the alignment. `1.0` for the common case.
    pub rate: f64,
    /// The query was under `MIN_QUERY_SECONDS`, so no comparison was possible.
    pub too_short: bool,
    /// The posting budget stopped the scan; the answer is "not established", not "not the same".
    pub budget_exceeded: bool,
}

impl Affirmation {
    const fn refused(too_short: bool, budget_exceeded: bool) -> Self {
        Self {
            affirmed: false,
            gates_passed: false,
            coverage: 0.0,
            regions_expected: 0,
            regions_covered: 0,
            unexplained_regions: 0,
            max_unexplained_gap_seconds: 0.0,
            offset_discontinuities: 0,
            duration_consistent: false,
            reference_duration_seconds: 0.0,
            query_duration_seconds: 0.0,
            coherence_ratio: 0.0,
            peak: 0,
            best_outside_peak: 0,
            aligned_span_seconds: 0.0,
            rate: 1.0,
            too_short,
            budget_exceeded,
        }
    }
}

/// The corroboration predicate, in full.
///
/// The reference is loaded into a ONE-track index and run through exactly the rung-5 search, rate
/// sweep included, so the acceptance predicate here and the one the false-positive measurement
/// priced are the same code path rather than two implementations of one idea.
pub fn compare(
    reference: &ReferenceFingerprint,
    query: &FingerprintQuery,
    limits: ScoreLimits,
) -> Result<Affirmation, TraceError> {
    // The index holds one track and the record it belongs to is already in hand, so the id is a
    // fixed placeholder rather than a lookup key.
    let record = RecordId::from_manifest_bytes(b"apw-trace-reference-fingerprint");
    let mut index = MemoryFingerprintIndex::new();
    index.insert_landmarks(0, record, &landmark::landmarks(&reference.constellation));

    let outcome = search(&index, query, limits)?;
    if outcome.too_short || outcome.budget_exceeded {
        return Ok(Affirmation::refused(
            outcome.too_short,
            outcome.budget_exceeded,
        ));
    }
    let (alignment, rate, gates_passed) = match (&outcome.hit, &outcome.best_seen) {
        (Some(hit), _) => (&hit.alignment, hit.rate, true),
        (None, Some(best)) => (best, 1.0, false),
        (None, None) => return Ok(Affirmation::refused(false, false)),
    };
    let locality = local_coverage(reference, query, alignment, rate);
    let coverage = if locality.regions_expected == 0 {
        0.0
    } else {
        locality.regions_covered as f64 / locality.regions_expected as f64
    };
    let gap_ok = locality.max_unexplained_gap_seconds <= MAX_UNEXPLAINED_GAP_SECONDS;
    let continuity_ok = alignment.local_offset_discontinuities == 0;
    Ok(Affirmation {
        affirmed: gates_passed
            && coverage >= AFFIRMATION_COVERAGE
            && gap_ok
            && continuity_ok
            && locality.duration_consistent,
        gates_passed,
        coverage,
        regions_expected: locality.regions_expected,
        regions_covered: locality.regions_covered,
        unexplained_regions: locality
            .regions_expected
            .saturating_sub(locality.regions_covered),
        max_unexplained_gap_seconds: locality.max_unexplained_gap_seconds,
        offset_discontinuities: alignment.local_offset_discontinuities,
        duration_consistent: locality.duration_consistent,
        reference_duration_seconds: reference.duration_seconds(),
        query_duration_seconds: query.seconds(),
        coherence_ratio: alignment.coherence_ratio,
        peak: alignment.peak,
        best_outside_peak: alignment.best_outside_peak,
        aligned_span_seconds: alignment.aligned_span_seconds,
        rate,
        too_short: false,
        budget_exceeded: false,
    })
}

#[derive(Debug, Clone, Copy)]
struct LocalCoverage {
    regions_expected: usize,
    regions_covered: usize,
    max_unexplained_gap_seconds: f64,
    duration_consistent: bool,
}

fn local_coverage(
    reference: &ReferenceFingerprint,
    query: &FingerprintQuery,
    alignment: &crate::fingerprint::score::TrackAlignment,
    rate: f64,
) -> LocalCoverage {
    use crate::fingerprint::landmark::FRAMES_PER_SECOND;

    let query_frames = (query.seconds() * FRAMES_PER_SECOND * rate)
        .round()
        .max(0.0) as i64;
    let reference_frames = reference.constellation.frames() as i64;
    let offset = alignment.peak_offset_frames;

    let mut reference_anchors: Vec<u32> = landmark::landmarks(&reference.constellation)
        .into_iter()
        .map(|landmark| landmark.t1)
        .collect();
    reference_anchors.sort_unstable();
    reference_anchors.dedup();

    local_coverage_from_frames(
        reference_frames,
        query_frames,
        offset,
        &reference_anchors,
        &alignment.aligned_query_frames,
    )
}

/// Pure local-association predicate. Its inputs are all signed reference facts or recovered
/// alignment facts, which makes the interior-gap and duration rules testable without an FFT or an
/// index. `reference_anchors` are the signed constellation's derived landmark distribution.
fn local_coverage_from_frames(
    reference_frames: i64,
    query_frames: i64,
    offset: i64,
    reference_anchors: &[u32],
    aligned_query_frames: &[u32],
) -> LocalCoverage {
    use crate::fingerprint::landmark::FRAMES_PER_SECOND;

    let region_frames = (AFFIRMATION_REGION_SECONDS * FRAMES_PER_SECOND)
        .round()
        .max(1.0) as i64;
    let tolerance_frames = (DURATION_MAPPING_TOLERANCE_SECONDS * FRAMES_PER_SECOND).ceil() as i64;
    let mapped_start = offset;
    let mapped_end = offset.saturating_add(query_frames);
    let duration_consistent = mapped_start >= -tolerance_frames
        && mapped_end <= reference_frames.saturating_add(tolerance_frames);

    let mut regions_expected = 0usize;
    let mut regions_covered = 0usize;
    let mut current_gap = 0usize;
    let mut max_gap = 0usize;
    let region_count = if query_frames <= 0 {
        0
    } else {
        ((query_frames + region_frames - 1) / region_frames) as usize
    };

    for region in 0..region_count {
        let query_low = region as i64 * region_frames;
        let query_high = ((region + 1) as i64 * region_frames).min(query_frames);
        let reference_low = query_low.saturating_add(offset);
        let reference_high = query_high.saturating_add(offset);
        let outside_reference = reference_low < 0 || reference_high > reference_frames;

        let expected_anchors = if outside_reference {
            MIN_REFERENCE_ANCHORS_PER_REGION
        } else {
            reference_anchors
                .iter()
                .filter(|frame| {
                    let frame = i64::from(**frame);
                    frame >= reference_low && frame < reference_high
                })
                .count()
        };
        if expected_anchors < MIN_REFERENCE_ANCHORS_PER_REGION {
            current_gap = 0;
            continue;
        }

        regions_expected += 1;
        let covered = !outside_reference
            && aligned_query_frames.iter().any(|frame| {
                let frame = i64::from(*frame);
                frame >= query_low && frame < query_high
            });
        if covered {
            regions_covered += 1;
            current_gap = 0;
        } else {
            current_gap += 1;
            max_gap = max_gap.max(current_gap);
        }
    }

    LocalCoverage {
        regions_expected,
        regions_covered,
        max_unexplained_gap_seconds: max_gap as f64 * AFFIRMATION_REGION_SECONDS,
        duration_consistent,
    }
}

fn malformed(reason: impl Into<String>) -> TraceError {
    TraceError::InvalidOption {
        option: "fingerprint",
        reason: reason.into(),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], TraceError> {
        let end = self
            .at
            .checked_add(count)
            .ok_or_else(|| malformed("reference length overflows"))?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| malformed("reference is truncated"))?;
        self.at = end;
        Ok(slice)
    }

    fn varint(&mut self) -> Result<u64, TraceError> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.take(1)?[0];
            value |= u64::from(byte & 0x7F)
                .checked_shl(shift)
                .ok_or_else(|| malformed("varint is over-long"))?;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(malformed("varint is over-long"))
    }

    const fn is_empty(&self) -> bool {
        self.at >= self.bytes.len()
    }
}

fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod local_association_tests {
    use super::*;
    use crate::fingerprint::landmark::FRAMES_PER_SECOND;

    fn dense_reference(seconds: f64) -> (i64, Vec<u32>) {
        let frames = (seconds * FRAMES_PER_SECOND).round() as i64;
        let region = (AFFIRMATION_REGION_SECONDS * FRAMES_PER_SECOND).round() as i64;
        let mut anchors = Vec::new();
        let mut low = 0i64;
        while low < frames {
            for fraction in [1i64, 2, 3] {
                let frame = low + fraction * region / 4;
                if frame < frames {
                    anchors.push(frame as u32);
                }
            }
            low += region;
        }
        (frames, anchors)
    }

    fn aligned_regions(query_frames: i64, omitted: core::ops::Range<usize>) -> Vec<u32> {
        let region = (AFFIRMATION_REGION_SECONDS * FRAMES_PER_SECOND).round() as i64;
        let count = ((query_frames + region - 1) / region) as usize;
        (0..count)
            .filter(|index| !omitted.contains(index))
            .map(|index| (index as i64 * region + region / 2) as u32)
            .collect()
    }

    #[test]
    fn every_predeclared_interior_replacement_fraction_breaks_the_gap_rule() {
        let (reference_frames, anchors) = dense_reference(100.0);
        for percent in [1usize, 2, 5, 10, 25, 50] {
            let region_count = (100.0 / AFFIRMATION_REGION_SECONDS) as usize;
            let missing = (region_count * percent / 100).max(1);
            let start = (region_count - missing) / 2;
            let locality = local_coverage_from_frames(
                reference_frames,
                reference_frames,
                0,
                &anchors,
                &aligned_regions(reference_frames, start..start + missing),
            );
            assert!(
                locality.max_unexplained_gap_seconds > MAX_UNEXPLAINED_GAP_SECONDS,
                "{percent}% interior replacement escaped: {locality:?}"
            );
        }
    }

    #[test]
    fn head_tail_crop_and_extra_duration_are_distinct() {
        let (reference_frames, anchors) = dense_reference(100.0);
        let count = (100.0 / AFFIRMATION_REGION_SECONDS) as usize;
        for missing in [0..2, count - 2..count] {
            let locality = local_coverage_from_frames(
                reference_frames,
                reference_frames,
                0,
                &anchors,
                &aligned_regions(reference_frames, missing),
            );
            assert!(locality.duration_consistent);
            assert!(locality.max_unexplained_gap_seconds > MAX_UNEXPLAINED_GAP_SECONDS);
        }

        let crop_offset = (3.37 * FRAMES_PER_SECOND).round() as i64;
        let crop_frames = (71.29 * FRAMES_PER_SECOND).round() as i64;
        let crop = local_coverage_from_frames(
            reference_frames,
            crop_frames,
            crop_offset,
            &anchors,
            &aligned_regions(crop_frames, 0..0),
        );
        assert!(crop.duration_consistent);
        assert_eq!(crop.regions_expected, crop.regions_covered);
        assert_eq!(crop.max_unexplained_gap_seconds, 0.0);

        let duplicated_tail_frames = reference_frames + (2.0 * FRAMES_PER_SECOND).round() as i64;
        let duplicate = local_coverage_from_frames(
            reference_frames,
            duplicated_tail_frames,
            0,
            &anchors,
            &aligned_regions(duplicated_tail_frames, 0..0),
        );
        assert!(!duplicate.duration_consistent);
    }
}
