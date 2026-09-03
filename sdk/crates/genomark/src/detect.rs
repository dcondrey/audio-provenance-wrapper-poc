use audio_provenance_audio::AudioBuffer;
use audio_provenance_core::{AssociationClaim, ProofLevel};

use crate::block::{SlotRole, slot_role};
use crate::convolutional::{decode, encode};
use crate::error::GenoMarkError;
use crate::geometry::Band;
use crate::interleaver::{deinterleave, interleave};
use crate::keystream::Schedule;
use crate::params::{
    BLOCK_FRAMES, BLOCK_SLOTS, BLOCKS_PER_EPOCH, COARSE_RHO_HALF_SPAN, COARSE_RHO_STEP, CODED_BITS,
    COMBINE_ANCHOR_HALF_SPAN, COMBINE_RHO_HALF_SPAN, DELTA, DETECT_HOP, FINE_RHO_HALF_SPAN,
    FINE_RHO_STEP, FRAMES_PER_SLOT, MAX_COMBINE_LATTICES, MAX_CRC_ATTEMPTS, MAX_SAMPLE_RATE,
    MAX_SYNC_CANDIDATES, NORMALIZER_HALF_WINDOW, PEAK_SUPPRESSION_SLOTS, PREAMBLE_SLOTS,
    SUB_PHASES, TAU_SYNC,
};
use crate::payload::Payload;
use crate::spectrogram::{PairDiffs, Spectrogram};
use crate::statistic::{SLOT_SAMPLES, lattice_quotient, median, soft_bit, trimmed_mean};
use crate::validate::admit;

/// Stride at which the sliding median and MAD are re-evaluated and held.
///
/// PERF: the normaliser feeds only the reliability weight and the correlation scale, both of which
/// move over seconds. Re-selecting a 129-sample median at every embed frame was the detector's
/// single largest cost and changed no decision.
const NORMALIZER_STRIDE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfidenceClass {
    /// Two or more non-overlapping blocks CRC-passed on their own with identical payloads.
    Strong,
    /// Exactly one block CRC-passed on its own, or the payload came only from the cross-block
    /// accumulator, where no single block carried enough evidence to pass the CRC alone.
    Single,
    None,
}

impl ConfidenceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Single => "single",
            Self::None => "none",
        }
    }
}

/// What a blind detection reports. There is no field for an expected payload and no entry point
/// that takes one: an oracle-gated detector measures the searcher, not the mark.
#[derive(Debug, Clone)]
pub struct DetectionOutcome {
    payload: Option<Payload>,
    class: ConfidenceClass,
    confidence: f64,
    blocks_accepted: usize,
    blocks_combined: usize,
    sync_candidates: usize,
    sync_cap_reached: bool,
    crc_attempts: usize,
    bits_corrected: u32,
    rate_factor: Option<f64>,
    namespace_mismatch: bool,
    /// Non-overlapping self-accepted block spans in presented-audio seconds. The distribution is
    /// reported alongside the count so a verifier can distinguish evenly recovered evidence from
    /// the same number of blocks clustered around one lifted region.
    accepted_spans_seconds: Vec<(f64, f64)>,
}

impl DetectionOutcome {
    pub const fn none(sync_candidates: usize, sync_cap_reached: bool, crc_attempts: usize) -> Self {
        Self {
            payload: None,
            class: ConfidenceClass::None,
            confidence: 0.0,
            blocks_accepted: 0,
            blocks_combined: 0,
            sync_candidates,
            sync_cap_reached,
            crc_attempts,
            bits_corrected: 0,
            rate_factor: None,
            namespace_mismatch: false,
            accepted_spans_seconds: Vec::new(),
        }
    }

    pub const fn payload(&self) -> Option<Payload> {
        self.payload
    }

    pub const fn class(&self) -> ConfidenceClass {
        self.class
    }

    pub const fn confidence(&self) -> f64 {
        self.confidence
    }

    /// Non-overlapping blocks that CRC-passed ON THEIR OWN and agreed with the reported payload.
    ///
    /// IMPORTANT: this is a per-block count and it stays one. `genotrace`'s block-coverage guard
    /// divides it by the whole-block count of the presented audio, and that ratio is what fails a
    /// marked insert spliced into unmarked material. Counting blocks that merely contributed soft
    /// evidence here would let the unmarked half of a splice raise its own coverage. Those are
    /// [`Self::blocks_combined`].
    pub const fn blocks_accepted(&self) -> usize {
        self.blocks_accepted
    }

    pub fn accepted_spans_seconds(&self) -> &[(f64, f64)] {
        &self.accepted_spans_seconds
    }

    /// Blocks whose per-coded-bit soft evidence was summed into the accumulated decode, zero when
    /// the payload came from a single block's own CRC pass. A contributing block is not a block
    /// that carried the mark: contribution is decided before any decode, so a block of unmarked
    /// audio inside the file is counted here and adds noise rather than evidence.
    pub const fn blocks_combined(&self) -> usize {
        self.blocks_combined
    }

    pub const fn sync_candidates(&self) -> usize {
        self.sync_candidates
    }

    pub const fn crc_attempts(&self) -> usize {
        self.crc_attempts
    }

    /// True when either cap was reached, meaning peaks above `TAU_SYNC` were dropped unexamined.
    /// The section 9 false-positive arithmetic still holds, but the recovery number for this input
    /// is a cap artifact rather than a measurement of the channel.
    ///
    /// The sync cap is counted as well as the CRC cap. Reaching `MAX_SYNC_CANDIDATES` discards
    /// above-threshold peaks before any decode is attempted, so a report that keyed only on CRC
    /// attempts called a capped search uncapped.
    pub const fn candidate_cap_reached(&self) -> bool {
        self.sync_cap_reached || self.crc_attempts >= MAX_CRC_ATTEMPTS
    }

    /// Set when an accepted payload names a namespace other than the one the detector was keyed
    /// for. Acceptance stays CRC-gated; this is reported, never used to reject.
    pub const fn namespace_mismatch(&self) -> bool {
        self.namespace_mismatch
    }

    pub const fn bits_corrected(&self) -> u32 {
        self.bits_corrected
    }

    pub const fn rate_factor(&self) -> Option<f64> {
        self.rate_factor
    }

    /// A recovered soft binding is evidence of association and nothing more. The vocabulary derives
    /// the proof level from the status, so there is no path here that promotes a single block above
    /// `inferred`.
    pub const fn association(&self) -> AssociationClaim {
        if self.payload.is_some() {
            AssociationClaim::established()
        } else {
            AssociationClaim::not_established()
        }
    }

    pub const fn proof_level(&self) -> ProofLevel {
        self.association().proof_level()
    }
}

#[derive(Debug, Clone, Copy)]
struct SyncCandidate {
    anchor: f64,
    /// Sub-phase the peak was found on, in detector frames.
    phase: f64,
    /// Embed-frame index of the peak within that sub-phase's series.
    start: usize,
    rho: f64,
    epoch: u64,
    score: f64,
}

struct Normalised {
    statistic: Vec<f64>,
    weight: Vec<f64>,
}

fn slot_statistics(diffs: &PairDiffs, anchor: f64, rho: f64, first: i64, count: usize) -> Vec<f64> {
    let pairs = diffs.pairs();
    let mut frame = vec![0.0f64; pairs];
    let mut per_frame = vec![f64::NAN; (count + 1) * pairs];
    for index in 0..=count {
        let embed = first + index as i64;
        let position = anchor + SUB_PHASES as f64 * embed as f64 / rho;
        if diffs.interpolate(position, &mut frame) {
            per_frame[index * pairs..(index + 1) * pairs].copy_from_slice(&frame);
        }
    }
    let mut values = Vec::with_capacity(SLOT_SAMPLES);
    (0..count)
        .map(|index| {
            values.clear();
            values.extend_from_slice(&per_frame[index * pairs..(index + 2) * pairs]);
            if values.iter().any(|value| !value.is_finite()) {
                return f64::NAN;
            }
            trimmed_mean(&mut values).unwrap_or(f64::NAN)
        })
        .collect()
}

fn normalise(statistic: Vec<f64>) -> Normalised {
    let count = statistic.len();
    let mut local = vec![f64::NAN; count];
    let mut scratch: Vec<f64> = Vec::with_capacity(2 * NORMALIZER_HALF_WINDOW + 1);
    let mut deviations: Vec<f64> = Vec::with_capacity(2 * NORMALIZER_HALF_WINDOW + 1);
    let mut index = 0usize;
    while index < count {
        let low = index.saturating_sub(NORMALIZER_HALF_WINDOW);
        let high = (index + NORMALIZER_HALF_WINDOW + 1).min(count);
        scratch.clear();
        scratch.extend(
            statistic[low..high]
                .iter()
                .copied()
                .filter(|v| v.is_finite()),
        );
        let value = match median(&mut scratch) {
            Some(centre) => {
                deviations.clear();
                deviations.extend(scratch.iter().map(|v| (v - centre).abs()));
                median(&mut deviations).unwrap_or(f64::NAN)
            }
            None => f64::NAN,
        };
        let end = (index + NORMALIZER_STRIDE).min(count);
        for slot in &mut local[index..end] {
            *slot = value;
        }
        index = end;
    }
    let mut finite: Vec<f64> = local
        .iter()
        .copied()
        .filter(|v| v.is_finite() && *v > 0.0)
        .collect();
    let global = median(&mut finite).unwrap_or(0.0);
    let weight = local
        .iter()
        .map(|&value| {
            if !value.is_finite() || global <= 0.0 {
                0.0
            } else {
                1.0 / (1.0 + (value / global).powi(2))
            }
        })
        .collect();
    Normalised { statistic, weight }
}

fn soft_value(statistic: f64, dither: f64, weight: f64) -> f64 {
    if !statistic.is_finite() {
        return 0.0;
    }
    weight * soft_bit(lattice_quotient(statistic, dither, DELTA))
}

/// Correlation of the keyed preamble against the QIM soft metric, in units of its own H0 standard
/// deviation. Under H0 the lattice position is uniform, so the metric is uniform on [-0.5, 0.5] and
/// its variance is 1/12; `TAU_SYNC` is then a per-hypothesis false-alarm rate, not a tuned knob.
fn preamble_score(normalised: &Normalised, schedule: &Schedule, start: usize) -> Option<f64> {
    let mut sum = 0.0f64;
    let mut power = 0.0f64;
    for (index, &polarity) in schedule.preamble().iter().enumerate() {
        let slot = start + index * FRAMES_PER_SLOT;
        let statistic = *normalised.statistic.get(slot)?;
        let weight = *normalised.weight.get(slot)?;
        sum += f64::from(polarity) * soft_value(statistic, schedule.dither(index), weight);
        power += weight * weight;
    }
    if power <= 0.0 {
        return None;
    }
    Some(sum / (power / 12.0).sqrt())
}

fn coarse_grid() -> Vec<f64> {
    let half = COARSE_RHO_HALF_SPAN as i64;
    (-half..=half)
        .map(|step| 1.0 + step as f64 * COARSE_RHO_STEP)
        .collect()
}

fn fine_grid(centre: f64) -> Vec<f64> {
    let half = FINE_RHO_HALF_SPAN as i64;
    (-half..=half)
        .map(|step| centre + step as f64 * FINE_RHO_STEP)
        .collect()
}

fn conform(audio: &AudioBuffer) -> Result<AudioBuffer, GenoMarkError> {
    let audio = admit(audio)?;
    if audio.sample_rate() > MAX_SAMPLE_RATE {
        return audio_provenance_audio::resample(&audio, crate::params::ANALYSIS_SAMPLE_RATE)
            .map_err(GenoMarkError::from);
    }
    Ok(audio)
}

struct BlockDecode {
    payload: Payload,
    bits_corrected: u32,
    rho: f64,
    span: (f64, f64),
}

fn block_series(diffs: &PairDiffs, anchor: f64, rho: f64) -> Normalised {
    let half = NORMALIZER_HALF_WINDOW as i64;
    let count = BLOCK_FRAMES + 2 * NORMALIZER_HALF_WINDOW;
    normalise(slot_statistics(diffs, anchor, rho, -half, count))
}

fn block_slot_index(slot: usize) -> usize {
    NORMALIZER_HALF_WINDOW + slot * FRAMES_PER_SLOT
}

fn pilot_score(statistic: &[f64], weight: &[f64], schedule: &Schedule) -> f64 {
    let mut score = 0.0f64;
    for slot in 0..BLOCK_SLOTS {
        let SlotRole::Pilot(pilot) = slot_role(slot) else {
            continue;
        };
        let Some(&polarity) = schedule.pilot().get(pilot) else {
            continue;
        };
        let index = block_slot_index(slot);
        let (Some(&value), Some(&w)) = (statistic.get(index), weight.get(index)) else {
            continue;
        };
        score += f64::from(polarity) * soft_value(value, schedule.dither(slot), w);
    }
    score
}

/// Viterbi over one per-coded-bit soft metric, then the CRC. The CRC inside `Payload::from_message`
/// is the only thing that accepts, whether the metric came from one block or from a sum over many.
fn decode_received(received: &[f64]) -> Option<(Payload, u32)> {
    let coded = deinterleave(received);
    let message = decode(&coded)?;
    let payload = Payload::from_message(&message)?;
    let reference = interleave(&encode(&payload.to_message()));
    let corrected = reference
        .iter()
        .zip(received.iter())
        .filter(|(expected, observed)| (**observed > 0.0) != (**expected == 1))
        .count() as u32;
    Some((payload, corrected))
}

fn attempt_decode(
    statistic: &[f64],
    weight: &[f64],
    schedule: &Schedule,
) -> Option<(Payload, u32)> {
    let mut received = vec![0.0f64; CODED_BITS];
    for slot in 0..BLOCK_SLOTS {
        let SlotRole::Data(position) = slot_role(slot) else {
            continue;
        };
        let index = block_slot_index(slot);
        let (Some(&value), Some(&w)) = (statistic.get(index), weight.get(index)) else {
            continue;
        };
        if let Some(cell) = received.get_mut(position) {
            *cell = soft_value(value, schedule.dither(slot), w);
        }
    }
    decode_received(&received)
}

/// Correlation of the preamble AND pilot slots of one block against the QIM soft metric.
///
/// IMPORTANT: preamble and pilot are keyed and known before any decode, so maximising this over a
/// block's alignment stays blind. Nothing here may read a data slot or a decode result: selecting
/// an alignment by anything on the data path is an oracle, and at the scale this accumulates over
/// it would manufacture the false positives the CRC gate is there to bound.
fn keyed_score(series: &Normalised, schedule: &Schedule, base: usize) -> Option<f64> {
    let mut score = 0.0f64;
    for slot in 0..BLOCK_SLOTS {
        let polarity = match slot_role(slot) {
            SlotRole::Preamble(index) => *schedule.preamble().get(index)?,
            SlotRole::Pilot(index) => *schedule.pilot().get(index)?,
            SlotRole::Data(_) => continue,
        };
        let index = base + slot * FRAMES_PER_SLOT;
        let statistic = *series.statistic.get(index)?;
        let weight = *series.weight.get(index)?;
        score += f64::from(polarity) * soft_value(statistic, schedule.dither(slot), weight);
    }
    Some(score)
}

/// Adds one block's per-coded-bit soft metric into the running sum.
fn accumulate_block(series: &Normalised, schedule: &Schedule, base: usize, received: &mut [f64]) {
    for slot in 0..BLOCK_SLOTS {
        let SlotRole::Data(position) = slot_role(slot) else {
            continue;
        };
        let index = base + slot * FRAMES_PER_SLOT;
        let (Some(&value), Some(&weight)) = (series.statistic.get(index), series.weight.get(index))
        else {
            continue;
        };
        if let Some(cell) = received.get_mut(position) {
            *cell += soft_value(value, schedule.dither(slot), weight);
        }
    }
}

/// One block grid: a sub-phase, a rate, an epoch, and one embed-frame index known to sit on a block
/// boundary. Every block of an epoch carries the same coded word under the same dither, so one
/// epoch's worth of blocks either side of that index is addressed by this alone.
#[derive(Debug, Clone, Copy)]
struct Lattice {
    phase: f64,
    start: usize,
    coarse_rho: f64,
    rho: f64,
    epoch: u64,
    score: f64,
}

struct CombinedDecode {
    payload: Payload,
    bits_corrected: u32,
    rho: f64,
    blocks: usize,
}

/// Sums the per-coded-bit soft metric over every block the lattice reaches, then runs the Viterbi
/// once over that sum and lets the CRC decide.
///
/// This is soft combining, not a vote: a block that individually falls short of the CRC still
/// contributes its evidence, which is the whole gain. Acceptance is unchanged, because the only
/// thing that ever accepts is the CRC over the single decoded message.
fn combine_lattice(
    diffs: &PairDiffs,
    frames: usize,
    schedule: &Schedule,
    lattice: &Lattice,
    rho: f64,
) -> Option<CombinedDecode> {
    let reach = ((frames as f64 - 2.0 - lattice.phase) * rho / SUB_PHASES as f64).floor();
    if !reach.is_finite() || reach <= BLOCK_FRAMES as f64 {
        return None;
    }
    let count = reach as usize;
    let series = normalise(slot_statistics(diffs, lattice.phase, rho, 0, count));

    // The same physical position carries a different embed-frame index once the rate moves, because
    // the index space IS the authored clock.
    let origin = (lattice.start as f64 * rho / lattice.coarse_rho).round() as i64;
    let period = BLOCK_FRAMES as i64;
    let span = ((BLOCK_SLOTS - 1) * FRAMES_PER_SLOT) as i64;

    let mut received = vec![0.0f64; CODED_BITS];
    let mut blocks = 0usize;
    // IMPORTANT: the schedule changes every BLOCKS_PER_EPOCH blocks and a blind detector cannot know
    // where in its epoch the anchor sits, so the reach is bounded to one epoch either side. Audio
    // longer than that accumulates over its own epoch and stops, rather than summing evidence
    // demodulated against the wrong dither.
    let limit = BLOCKS_PER_EPOCH as i64;
    for index in -limit..=limit {
        let predicted = origin + index * period;
        // The re-anchor window is clipped rather than the block dropped, so the first and last
        // blocks of a file still contribute. `keyed_score` returns None for a base whose block runs
        // off the end, which is what bounds the search at both edges.
        if predicted < 0 || predicted + span >= count as i64 {
            continue;
        }
        let mut best: Option<(f64, usize)> = None;
        for offset in -COMBINE_ANCHOR_HALF_SPAN..=COMBINE_ANCHOR_HALF_SPAN {
            if predicted + offset < 0 {
                continue;
            }
            let base = (predicted + offset) as usize;
            let Some(score) = keyed_score(&series, schedule, base) else {
                continue;
            };
            if best.is_none_or(|(kept, _)| score > kept) {
                best = Some((score, base));
            }
        }
        let Some((_, base)) = best else {
            continue;
        };
        accumulate_block(&series, schedule, base, &mut received);
        blocks += 1;
    }
    if blocks == 0 {
        return None;
    }
    let (payload, bits_corrected) = decode_received(&received)?;
    Some(CombinedDecode {
        payload,
        bits_corrected,
        rho,
        blocks,
    })
}

/// True when two grids address the same blocks, so accumulating from either would sum the same
/// evidence and the second is a wasted CRC gate.
fn same_lattice(left: &Lattice, right: &Lattice) -> bool {
    if left.epoch != right.epoch
        || (left.coarse_rho - right.coarse_rho).abs() >= COARSE_RHO_STEP / 2.0
    {
        return false;
    }
    let left_anchor = left.phase + SUB_PHASES as f64 * left.start as f64 / left.coarse_rho;
    let right_anchor = right.phase + SUB_PHASES as f64 * right.start as f64 / right.coarse_rho;
    let period = SUB_PHASES as f64 * BLOCK_FRAMES as f64 / left.coarse_rho;
    if period <= 0.0 || !period.is_finite() {
        return false;
    }
    let delta = (left_anchor - right_anchor).abs() % period;
    let wrapped = delta.min(period - delta);
    wrapped <= (COMBINE_ANCHOR_HALF_SPAN * SUB_PHASES as i64) as f64
}

pub(crate) fn detect_in(
    profile_key: &[u8],
    namespace: u8,
    audio: &AudioBuffer,
) -> Result<DetectionOutcome, GenoMarkError> {
    let audio = conform(audio)?;
    let band = Band::new(audio.sample_rate())?;
    let mid = audio.mono_sum();

    let coarse = coarse_grid();
    let fine_reach = FINE_RHO_HALF_SPAN as f64 * FINE_RHO_STEP;
    let low_rho = coarse.first().copied().unwrap_or(1.0) - fine_reach;
    let high_rho = coarse.last().copied().unwrap_or(1.0) + fine_reach;
    let (low_bin, high_bin) = band.spectrogram_span(&[low_rho, high_rho])?;
    let spectrogram = Spectrogram::analyse(&mid, band.frame(), DETECT_HOP, low_bin, high_bin)?;

    // Under one block of contiguous audio there is nothing to decode. The documented answer is
    // nothing, not an error: `not_found` never means no mark exists.
    let embed_frames = spectrogram.frames() / SUB_PHASES;
    if embed_frames < BLOCK_FRAMES + 2 * NORMALIZER_HALF_WINDOW {
        return Ok(DetectionOutcome::none(0, false, 0));
    }
    let epochs = (embed_frames / (BLOCK_FRAMES * BLOCKS_PER_EPOCH) + 1) as u64;
    let mut schedules = Vec::new();
    for epoch in 0..epochs {
        schedules.push(Schedule::derive(profile_key, namespace, epoch)?);
    }

    let active = band.active_pairs().to_vec();
    let mut candidates: Vec<SyncCandidate> = Vec::new();
    let mut grids: Vec<Lattice> = Vec::new();
    let preamble_span = (PREAMBLE_SLOTS - 1) * FRAMES_PER_SLOT;

    for &rho in &coarse {
        let edges = band.scaled_edges(rho)?;
        let diffs = PairDiffs::compute(&spectrogram, &edges, &active, 0, spectrogram.frames());
        for phase in 0..SUB_PHASES {
            let anchor = phase as f64;
            let reach =
                ((spectrogram.frames() as f64 - 2.0 - anchor) * rho / SUB_PHASES as f64).floor();
            if !reach.is_finite() || reach <= preamble_span as f64 {
                continue;
            }
            let count = reach as usize;
            let statistic = slot_statistics(&diffs, anchor, rho, 0, count);
            let normalised = normalise(statistic);
            for (epoch, schedule) in schedules.iter().enumerate() {
                // The block grid, folded. Every block of an epoch repeats the same preamble at the
                // same residue modulo BLOCK_FRAMES, so summing the correlation over the residue
                // class integrates the sync statistic across the whole file the same way the data
                // path integrates the coded bits. Under H0 each term is one standard deviation, so
                // the sum of n of them is compared against TAU_SYNC after dividing by sqrt(n) and
                // the per-hypothesis false-alarm rate the threshold encodes is unchanged.
                let mut folded = vec![0.0f64; BLOCK_FRAMES];
                let mut folded_count = vec![0u32; BLOCK_FRAMES];
                let mut folded_peak = vec![f64::NEG_INFINITY; BLOCK_FRAMES];
                for start in 0..count.saturating_sub(preamble_span) {
                    let Some(score) = preamble_score(&normalised, schedule, start) else {
                        continue;
                    };
                    if score > TAU_SYNC {
                        candidates.push(SyncCandidate {
                            anchor: anchor + SUB_PHASES as f64 * start as f64 / rho,
                            phase: anchor,
                            start,
                            rho,
                            epoch: epoch as u64,
                            score,
                        });
                    }
                    let cell = start % BLOCK_FRAMES;
                    folded[cell] += score;
                    folded_count[cell] += 1;
                    folded_peak[cell] = folded_peak[cell].max(score);
                }
                // Only the best few residues of this hypothesis are kept. A host that puts every
                // residue over the threshold would otherwise size an allocation from its own
                // content, and nothing below the top of one hypothesis can win the global ranking
                // that follows.
                let mut best: Vec<(f64, usize)> = Vec::with_capacity(MAX_COMBINE_LATTICES + 1);
                for cell in 0..BLOCK_FRAMES {
                    let repeats = folded_count[cell];
                    if repeats == 0 {
                        continue;
                    }
                    let integrated = folded[cell] / f64::from(repeats).sqrt();
                    // A grid earns a look if its integrated correlation clears the threshold OR one
                    // of its blocks does on its own. Integration alone would drop a file where a
                    // single block carries the mark; the peak alone is what the search already had.
                    let score = integrated.max(folded_peak[cell]);
                    if score <= TAU_SYNC {
                        continue;
                    }
                    best.push((score, cell));
                    best.sort_by(|a, b| {
                        b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal)
                    });
                    best.truncate(MAX_COMBINE_LATTICES);
                }
                grids.extend(best.into_iter().map(|(score, cell)| Lattice {
                    phase: anchor,
                    start: cell,
                    coarse_rho: rho,
                    rho,
                    epoch: epoch as u64,
                    score,
                }));
            }
        }
    }

    let sync_candidates = candidates.len();
    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    let suppression = (PEAK_SUPPRESSION_SLOTS * FRAMES_PER_SLOT * SUB_PHASES) as f64;
    let mut retained: Vec<SyncCandidate> = Vec::new();
    let mut sync_cap_reached = false;
    for candidate in candidates {
        if retained.len() >= MAX_SYNC_CANDIDATES {
            sync_cap_reached = true;
            break;
        }
        // IMPORTANT: suppression merges the eight sub-phases of one block, which differ only in
        // frame offset. It must NOT merge across coarse rate hypotheses. Over the 0.74 s preamble a
        // hypothesis one coarse step off the truth still correlates, and if it merely outscores the
        // nearest one it captures the candidate and carries a fine grid, +-half a coarse step wide,
        // that cannot reach the true rate. That is how a 0.1% drift decoded in one direction and
        // not the other.
        if retained.iter().any(|kept| {
            (kept.anchor - candidate.anchor).abs() < suppression
                && kept.epoch == candidate.epoch
                && (kept.rho - candidate.rho).abs() < COARSE_RHO_STEP / 2.0
        }) {
            continue;
        }
        retained.push(candidate);
    }

    let mut accepted: Vec<BlockDecode> = Vec::new();
    let mut best_fine: Vec<f64> = Vec::with_capacity(retained.len());
    let mut crc_attempts = 0usize;
    'candidates: for candidate in &retained {
        let Some(schedule) = schedules.get(candidate.epoch as usize) else {
            continue;
        };
        let mut scored: Vec<(f64, f64, Vec<f64>, Vec<f64>)> = Vec::new();
        let edges = band.scaled_edges(candidate.rho)?;
        let first = (candidate.anchor
            - SUB_PHASES as f64 * NORMALIZER_HALF_WINDOW as f64 / candidate.rho)
            .floor()
            .max(0.0) as usize;
        let last = (candidate.anchor
            + SUB_PHASES as f64 * (BLOCK_FRAMES + NORMALIZER_HALF_WINDOW + 2) as f64
                / candidate.rho)
            .ceil() as usize
            + 16;
        let diffs = PairDiffs::compute(&spectrogram, &edges, &active, first, last);
        for rho in fine_grid(candidate.rho) {
            let series = block_series(&diffs, candidate.anchor, rho);
            let score = pilot_score(&series.statistic, &series.weight, schedule);
            scored.push((score, rho, series.statistic, series.weight));
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(core::cmp::Ordering::Equal));
        best_fine.push(scored.first().map_or(candidate.rho, |entry| entry.1));
        for (_, rho, statistic, weight) in scored.into_iter().take(2) {
            if crc_attempts >= MAX_CRC_ATTEMPTS {
                break 'candidates;
            }
            crc_attempts += 1;
            if let Some((payload, corrected)) = attempt_decode(&statistic, &weight, schedule) {
                let end = candidate.anchor + SUB_PHASES as f64 * BLOCK_FRAMES as f64 / rho;
                accepted.push(BlockDecode {
                    payload,
                    bits_corrected: corrected,
                    rho,
                    span: (candidate.anchor, end),
                });
            }
        }
    }

    // Cross-block soft combining. It runs only once no single block has passed the CRC on its own,
    // for two reasons: a per-block pass already answers, and `blocks_accepted` keeps its per-block
    // meaning for the coverage guard that reads it.
    if accepted.is_empty() {
        grids.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        let mut lattices: Vec<Lattice> = Vec::new();
        for grid in &grids {
            if lattices.len() >= MAX_COMBINE_LATTICES {
                break;
            }
            if lattices.iter().any(|kept| same_lattice(kept, grid)) {
                continue;
            }
            // A retained peak on this grid already paid for a fine rate search; reuse its answer
            // rather than accumulating at the coarse rate.
            let mut lattice = *grid;
            for (index, candidate) in retained.iter().enumerate() {
                let peak = Lattice {
                    phase: candidate.phase,
                    start: candidate.start,
                    coarse_rho: candidate.rho,
                    rho: candidate.rho,
                    epoch: candidate.epoch,
                    score: candidate.score,
                };
                if same_lattice(&peak, grid) {
                    lattice.rho = best_fine.get(index).copied().unwrap_or(grid.rho);
                    break;
                }
            }
            lattices.push(lattice);
        }

        let mut combined: Option<CombinedDecode> = None;
        'lattices: for lattice in &lattices {
            let Some(schedule) = schedules.get(lattice.epoch as usize) else {
                continue;
            };
            let edges = band.scaled_edges(lattice.coarse_rho)?;
            let diffs = PairDiffs::compute(&spectrogram, &edges, &active, 0, spectrogram.frames());
            for step in -COMBINE_RHO_HALF_SPAN..=COMBINE_RHO_HALF_SPAN {
                if crc_attempts >= MAX_CRC_ATTEMPTS {
                    break 'lattices;
                }
                crc_attempts += 1;
                let rho = lattice.rho + step as f64 * FINE_RHO_STEP;
                if let Some(decoded) =
                    combine_lattice(&diffs, spectrogram.frames(), schedule, lattice, rho)
                {
                    combined = Some(decoded);
                    break 'lattices;
                }
            }
        }

        let Some(decoded) = combined else {
            return Ok(DetectionOutcome::none(
                sync_candidates,
                sync_cap_reached,
                crc_attempts,
            ));
        };
        let error_rate = f64::from(decoded.bits_corrected) / CODED_BITS as f64;
        return Ok(DetectionOutcome {
            namespace_mismatch: decoded.payload.namespace() != namespace,
            payload: Some(decoded.payload),
            class: ConfidenceClass::Single,
            confidence: (0.9 - error_rate).clamp(0.6, 0.9),
            blocks_accepted: 0,
            blocks_combined: decoded.blocks,
            sync_candidates,
            sync_cap_reached,
            crc_attempts,
            bits_corrected: decoded.bits_corrected,
            rate_factor: Some(decoded.rho),
            accepted_spans_seconds: Vec::new(),
        });
    }

    let best = accepted
        .iter()
        .min_by_key(|entry| entry.bits_corrected)
        .map(|entry| entry.payload);
    let Some(payload) = best else {
        return Ok(DetectionOutcome::none(
            sync_candidates,
            sync_cap_reached,
            crc_attempts,
        ));
    };
    let agreeing: Vec<&BlockDecode> = accepted
        .iter()
        .filter(|entry| entry.payload == payload)
        .collect();
    let mut disjoint: Vec<(f64, f64)> = Vec::new();
    for entry in &agreeing {
        if disjoint
            .iter()
            .any(|(low, high)| entry.span.0 < *high && *low < entry.span.1)
        {
            continue;
        }
        disjoint.push(entry.span);
    }
    let class = if disjoint.len() >= 2 {
        ConfidenceClass::Strong
    } else {
        ConfidenceClass::Single
    };
    let corrected = agreeing
        .iter()
        .map(|entry| entry.bits_corrected)
        .min()
        .unwrap_or(0);
    let error_rate = f64::from(corrected) / CODED_BITS as f64;
    let confidence = match class {
        ConfidenceClass::Strong => (0.99 - error_rate).clamp(0.95, 0.99),
        ConfidenceClass::Single => (0.9 - error_rate).clamp(0.6, 0.9),
        ConfidenceClass::None => 0.0,
    };

    let seconds_per_frame = DETECT_HOP as f64 / f64::from(audio.sample_rate());
    let accepted_spans_seconds = disjoint
        .iter()
        .map(|(low, high)| (low * seconds_per_frame, high * seconds_per_frame))
        .collect();
    Ok(DetectionOutcome {
        payload: Some(payload),
        class,
        confidence,
        blocks_accepted: disjoint.len(),
        blocks_combined: 0,
        sync_candidates,
        sync_cap_reached,
        crc_attempts,
        bits_corrected: corrected,
        rate_factor: agreeing.first().map(|entry| entry.rho),
        namespace_mismatch: payload.namespace() != namespace,
        accepted_spans_seconds,
    })
}
