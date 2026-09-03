//! Every free parameter GENOMARK_SPEC.md fixes, in one place.
//!
//! Frequencies are the authority; bin indices are derived from `fs` and the transform length at
//! run time (spec section 1), never stored.

pub const ALGORITHM_ID: &str = "genomark-lepqim-v1";

pub const REFERENCE_SAMPLE_RATE: f64 = 44_100.0;
pub const REFERENCE_FRAME: usize = 1024;
pub const REFERENCE_BAND_FIRST_BIN: usize = 20;

pub const MIN_SAMPLE_RATE: u32 = 32_000;
pub const MAX_SAMPLE_RATE: u32 = 96_000;
pub const ANALYSIS_SAMPLE_RATE: u32 = 48_000;
pub const LONG_FRAME_ABOVE: u32 = 52_000;

pub const CELLS: usize = 40;
pub const PAIRS: usize = 20;
pub const ACTIVE_PAIRS: usize = 14;
pub const GUARD_HZ: [f64; 6] = [1000.0, 1200.0, 2000.0, 2500.0, 3000.0, 3500.0];

pub const FRAMES_PER_SLOT: usize = 2;
pub const TRIM_EACH_TAIL: usize = 2;
pub const DELTA: f64 = 0.8;

pub const PUNCTURE_BAND_POWER: f64 = 1e-7;
pub const PUNCTURE_CELL_POWER: f64 = 1e-12;
pub const PUNCTURE_MIN_PAIRS: usize = 10;
pub const OLA_RESIDUAL_LIMIT: f64 = DELTA / 20.0;

pub const PREAMBLE_SLOTS: usize = 32;
pub const BODY_SLOTS: usize = 384;
pub const BLOCK_SLOTS: usize = PREAMBLE_SLOTS + BODY_SLOTS;
pub const PILOT_PERIOD: usize = 4;
pub const PILOT_SLOTS: usize = BODY_SLOTS / PILOT_PERIOD;
pub const DATA_SLOTS: usize = BODY_SLOTS - PILOT_SLOTS;
pub const BLOCK_FRAMES: usize = BLOCK_SLOTS * FRAMES_PER_SLOT;
pub const BLOCKS_PER_EPOCH: usize = 256;

pub const PAYLOAD_BITS: usize = 56;
pub const PAYLOAD_BYTES: usize = PAYLOAD_BITS / 8;
pub const CRC_BITS: usize = 32;
pub const TAIL_BITS: usize = 8;
pub const MESSAGE_BITS: usize = PAYLOAD_BITS + CRC_BITS;
pub const TRELLIS_STEPS: usize = MESSAGE_BITS + TAIL_BITS;
pub const CODE_RATE_DENOMINATOR: usize = 3;
pub const CODED_BITS: usize = TRELLIS_STEPS * CODE_RATE_DENOMINATOR;
pub const CONSTRAINT_LENGTH: usize = 9;
pub const TRELLIS_STATES: usize = 1 << (CONSTRAINT_LENGTH - 1);
pub const GENERATORS: [u16; CODE_RATE_DENOMINATOR] = [0o557, 0o663, 0o711];

/// Written row-wise, read column-wise. The spec's prose says "24 x 12" but also fixes the property
/// the interleaver exists for: adjacent coded bits land 12 slots apart (279 ms at 43.07 slots/s).
/// 12 rows of 24 satisfies that; 24 rows of 12 gives 24. The property is the authority.
pub const INTERLEAVER_ROWS: usize = 12;
pub const INTERLEAVER_COLS: usize = CODED_BITS / INTERLEAVER_ROWS;

pub const DETECT_HOP: usize = 64;
pub const SUB_PHASES: usize = 8;
/// Coarse rate step.
///
/// The spec derives 7.8e-3 from the preamble's coherent TIME span alone. Measured, that is far too
/// coarse in the FREQUENCY dimension: at 7.8e-3 the residual misplaces the top cell boundary by
/// 0.4 of a bin out of a 2-bin cell, and the preamble never crosses tau_sync outside about +-0.1%.
/// This step holds the worst-case boundary error near an eighth of a bin, which is what actually
/// makes the rate search work.
pub const COARSE_RHO_STEP: f64 = 2.5e-3;
pub const COARSE_RHO_HALF_SPAN: usize = 4;

/// Rate range this build actually searches, as a fraction. The spec predicts +-6% support from the
/// rate grid; searching further than this costs false-positive surface for hypotheses that cannot
/// decode, because a rate that far off decoheres the block long before the grid runs out.
pub const SEARCHED_RATE_RANGE: f64 = COARSE_RHO_STEP * COARSE_RHO_HALF_SPAN as f64;

/// Largest playback-rate deviation the bench matrix applies that the mark recovered through, as a
/// fraction, over the whole corpus and every channel.
///
/// It is deliberately not the widest rate any probe has recovered at. The matrix's drift rows are
/// +-0.1%, so +-0.1% is what a campaign has measured; a hand probe over three real corpus items
/// recovers further than that and is not a campaign.
pub const MEASURED_RATE_RANGE: f64 = 1.0e-3;

/// Fine rate step, applied to the TIME axis only.
///
/// The coherent span of one block is BLOCK_FRAMES embed frames, so a rate error of `d` displaces
/// the last slot of a block by `BLOCK_FRAMES * d` frames relative to its first. Measured on real
/// corpus audio, the keyed correlation falls to 46% of peak at 0.27 frames of end displacement and
/// to 23% at 0.54, so the usable budget is well under half a frame. A half step of 1.63e-4 puts the
/// worst case at 0.14 frames, and the measured decode window either side of the true rate is
/// +-5.5e-4, which this step covers with three grid points to spare.
pub const FINE_RHO_STEP: f64 = 3.26e-4;
pub const FINE_RHO_HALF_SPAN: usize = 12;
pub const NORMALIZER_HALF_WINDOW: usize = 64;
pub const TAU_SYNC: f64 = 4.27;
pub const MAD_SCALE: f64 = 1.4826;

/// Caps that keep the section 9 false-positive arithmetic true. A detect that examined more
/// candidates than section 9 counted would have a false-accept rate the spec never bounded.
pub const MAX_SYNC_CANDIDATES: usize = 32;
pub const MAX_CRC_ATTEMPTS: usize = 256;
pub const PEAK_SUPPRESSION_SLOTS: usize = 8;

/// Distinct block lattices the cross-block accumulator is allowed to try.
///
/// Every block of an epoch carries the same coded word under the same dither, so two sync
/// candidates that sit on the same lattice, one block period apart, produce the same accumulation.
/// Deduplicating by lattice is what keeps this to a handful of extra CRC gates rather than one per
/// candidate.
pub const MAX_COMBINE_LATTICES: usize = 2;

/// Fine rate steps either side of a lattice's best per-block rate that the accumulator re-tries.
pub const COMBINE_RHO_HALF_SPAN: i64 = 1;

/// Embed frames either side of the predicted start each block may be re-anchored by.
///
/// A rate error of half a fine step displaces the predicted start of block k by k * BLOCK_FRAMES *
/// FINE_RHO_STEP / 2 embed frames; over the 20 blocks of a four-minute track that is 2.7 frames.
/// This window covers it with margin and is far short of the half block period that would let a
/// block lock onto its neighbour.
pub const COMBINE_ANCHOR_HALF_SPAN: i64 = 8;

pub const PUBLIC_PROFILE_KEY: &[u8] = b"audio-provenance/genomark/public/v1";
