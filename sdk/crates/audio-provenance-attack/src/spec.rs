use audio_provenance_bench::attacks::erase::residue;
use audio_provenance_bench::attacks::geometry::{PairBandGeometry, PunctureRule};
use genomark::block::{SlotRole, slot_role};
use genomark::convolutional::encode;
use genomark::geometry::{Band, CELL_EDGES, cell_edge_hz};
use genomark::params::{BLOCK_SLOTS, DELTA, FRAMES_PER_SLOT, TRIM_EACH_TAIL};
use genomark::payload::Payload;

/// Everything a spec reader needs to compute the statistic the mark is quantised in.
///
/// Every value here is published in GENOMARK_SPEC.md section 1 and section 4. None of it depends on
/// the profile key.
pub fn geometry(sample_rate: u32) -> Result<PairBandGeometry, String> {
    let band = Band::new(sample_rate).map_err(|error| error.to_string())?;
    let edges: Vec<f64> = (0..CELL_EDGES).map(cell_edge_hz).collect();
    PairBandGeometry::new(
        sample_rate,
        band.frame(),
        FRAMES_PER_SLOT,
        TRIM_EACH_TAIL,
        BLOCK_SLOTS,
        DELTA,
        edges,
        band.active_pairs().to_vec(),
    )
    .map_err(|error| error.to_string())
}

/// The puncture rule the detector applies, copied out of the same published section.
pub fn puncture() -> PunctureRule {
    PunctureRule::new(
        genomark::params::PUNCTURE_BAND_POWER,
        genomark::params::PUNCTURE_CELL_POWER,
        genomark::params::PUNCTURE_MIN_PAIRS,
    )
}

/// The coded bit each slot of a block carries for a given payload, for the slots whose bit is
/// payload-derived. Preamble and pilot slots return `None`: their polarity comes from the key.
pub fn payload_slot_bits(payload: Payload) -> Vec<Option<u8>> {
    let interleaved = genomark::interleaver::interleave(&encode(&payload.to_message()));
    (0..BLOCK_SLOTS)
        .map(|slot| match slot_role(slot) {
            SlotRole::Data(index) => interleaved.get(index).copied(),
            SlotRole::Preamble(_) | SlotRole::Pilot(_) => None,
        })
        .collect()
}

/// Recovers the key-derived dither of every data slot from one marked file whose payload is known,
/// and returns the coset residue a chosen NEW payload would occupy.
///
/// The marked statistic of slot `s` is congruent to `dither[s] + bit[s] * DELTA/2` modulo `DELTA`.
/// Knowing `bit[s]` therefore yields `dither[s]`, and a different payload's bits can be written
/// against it. Preamble and pilot slots carry key-derived polarity that no payload changes, so
/// their observed residue is reused unchanged and never needs to be separated into its parts.
pub fn forged_residues(
    observed: &[Option<f64>],
    known: Payload,
    forged: Payload,
) -> Vec<Option<f64>> {
    let known_bits = payload_slot_bits(known);
    let forged_bits = payload_slot_bits(forged);
    observed
        .iter()
        .enumerate()
        .map(|(slot, value)| {
            let seen = (*value)?;
            match (
                known_bits.get(slot).copied().flatten(),
                forged_bits.get(slot).copied().flatten(),
            ) {
                (Some(old), Some(new)) => {
                    let dither = seen - f64::from(old) * DELTA / 2.0;
                    Some(residue(dither + f64::from(new) * DELTA / 2.0, DELTA))
                }
                _ => Some(seen),
            }
        })
        .collect()
}
