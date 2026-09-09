#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Known-answer and structural vectors. Each pins something a later change could silently break
//! without any round trip noticing.

use apw_watermark::block::{SlotRole, slot_role};
use apw_watermark::convolutional::{decode, encode};
use apw_watermark::crc32c::crc32c;
use apw_watermark::geometry::{Band, cell_edge_hz};
use apw_watermark::interleaver::{deinterleave, interleave, source_of};
use apw_watermark::params::{
    ACTIVE_PAIRS, BLOCK_SLOTS, CODED_BITS, DATA_SLOTS, INTERLEAVER_ROWS, MESSAGE_BITS, PILOT_SLOTS,
    PREAMBLE_SLOTS,
};
use apw_watermark::payload::Payload;
use apw_watermark::statistic::trimmed_mean;

/// The check-value every CRC-32C implementation agrees on. Without it a lookalike polynomial or an
/// unreflected table would round-trip against itself and gate nothing.
#[test]
fn crc32c_matches_the_published_check_value() {
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(crc32c(b""), 0x0000_0000);
    assert_eq!(crc32c(&[0u8; 32]), 0x8A91_36AA);
}

/// The spec states the band as bins 20..99 at 44.1 kHz with a 1024-sample frame and names the six
/// guarded pairs. Every other rate derives its bins from the same frequencies, so this is the only
/// place the stated numbers are checkable.
#[test]
fn reference_geometry_reproduces_the_spec_band_and_guard_mask() {
    let band = Band::new(44_100).unwrap();
    assert_eq!(band.frame(), 1024);
    let edges = band.edges(1.0).unwrap();
    for (index, edge) in edges.iter().enumerate() {
        assert_eq!(*edge, 20 + 2 * index, "cell edge {index}");
    }
    assert_eq!(band.active_pairs().len(), ACTIVE_PAIRS);
    let guarded: Vec<usize> = (0..20)
        .filter(|pair| !band.active_pairs().contains(pair))
        .collect();
    assert_eq!(guarded, vec![0, 1, 6, 9, 12, 15]);
    assert!((cell_edge_hz(0) - 861.32).abs() < 0.01);
    assert!((cell_edge_hz(40) - 4306.64).abs() < 0.01);
}

/// Every cell has to hold at least one bin at every supported rate, or the statistic reads a zero
/// energy and the log floor takes over.
#[test]
fn cell_geometry_is_non_degenerate_across_the_supported_range() {
    for rate in [32_000u32, 44_100, 48_000, 52_000, 52_001, 88_200, 96_000] {
        let band = Band::new(rate).unwrap();
        for rho in [0.9376f64, 1.0, 1.0624] {
            let edges = band.edges(rho).unwrap();
            for cell in 0..40 {
                assert!(
                    edges[cell + 1] > edges[cell],
                    "cell {cell} is empty at {rate} Hz, rho {rho}"
                );
            }
        }
    }
    assert!(Band::new(31_999).is_err());
    assert!(Band::new(96_001).is_err());
}

/// The closed form depends on this and nothing else: shift every input by the same amount and the
/// trimmed mean shifts by exactly that amount, because the ordering, and therefore which values are
/// dropped, is unchanged.
#[test]
fn trimmed_mean_shifts_by_exactly_a_uniform_shift() {
    let base: Vec<f64> = (0..28).map(|k| (k as f64 * 0.37).sin() * 2.5).collect();
    let reference = trimmed_mean(&mut base.clone()).unwrap();
    for shift in [-0.4f64, -0.05, 0.0, 0.2, 0.4] {
        let mut shifted: Vec<f64> = base.iter().map(|value| value + shift).collect();
        let moved = trimmed_mean(&mut shifted).unwrap();
        assert!((moved - reference - shift).abs() < 1e-12, "shift {shift}");
    }
}

#[test]
fn block_layout_matches_the_rate_budget() {
    let mut preamble = 0;
    let mut pilots = 0;
    let mut data: Vec<usize> = Vec::new();
    for slot in 0..BLOCK_SLOTS {
        match slot_role(slot) {
            SlotRole::Preamble(_) => preamble += 1,
            SlotRole::Pilot(_) => pilots += 1,
            SlotRole::Data(index) => data.push(index),
        }
    }
    assert_eq!(preamble, PREAMBLE_SLOTS);
    assert_eq!(pilots, PILOT_SLOTS);
    assert_eq!(data.len(), DATA_SLOTS);
    assert_eq!(data, (0..DATA_SLOTS).collect::<Vec<_>>());
}

/// The interleaver exists to put a burst error on non-adjacent coded bits. The spec fixes the
/// separation at 12 slots, which is 279 ms at 43 slots per second.
#[test]
fn interleaver_is_a_bijection_that_separates_adjacent_coded_bits() {
    let mut seen = vec![false; CODED_BITS];
    for position in 0..CODED_BITS {
        let source = source_of(position);
        assert!(!seen[source], "coded bit {source} is transmitted twice");
        seen[source] = true;
    }
    let mut position_of = vec![0usize; CODED_BITS];
    for position in 0..CODED_BITS {
        position_of[source_of(position)] = position;
    }
    for coded in 0..CODED_BITS - 1 {
        if position_of[coded] < position_of[coded + 1] {
            assert_eq!(
                position_of[coded + 1] - position_of[coded],
                INTERLEAVER_ROWS
            );
        }
    }
    let coded: Vec<u8> = (0..CODED_BITS).map(|index| (index % 2) as u8).collect();
    let wire = interleave(&coded);
    let soft: Vec<f64> = wire
        .iter()
        .map(|bit| if *bit == 1 { 1.0 } else { -1.0 })
        .collect();
    let recovered = deinterleave(&soft);
    for (index, value) in recovered.iter().enumerate() {
        assert_eq!(*value > 0.0, coded[index] == 1);
    }
}

/// The Viterbi has to correct real errors, not just pass a noiseless codeword through. A K=9
/// rate-1/3 code carries an 88-bit message through a burst plus scattered sign flips.
#[test]
fn soft_viterbi_corrects_a_burst_and_scattered_errors() {
    let payload = Payload::new(1, 5, 0xDEAD_BEEF_1234).unwrap();
    let message = payload.to_message();
    assert_eq!(message.len(), MESSAGE_BITS);
    let coded = encode(&message);
    assert_eq!(coded.len(), CODED_BITS);

    let mut soft: Vec<f64> = coded
        .iter()
        .map(|bit| if *bit == 1 { 1.0 } else { -1.0 })
        .collect();
    assert_eq!(decode(&soft).unwrap(), message);

    for value in &mut soft[40..52] {
        *value = -*value * 0.4;
    }
    for index in [3usize, 97, 150, 201, 262] {
        soft[index] = -soft[index];
    }
    let decoded = decode(&soft).unwrap();
    assert_eq!(decoded, message);
    assert_eq!(Payload::from_message(&decoded), Some(payload));
}

/// A message whose CRC does not check is discarded silently. Nothing downstream may see it.
#[test]
fn crc_gates_acceptance() {
    let payload = Payload::new(1, 0, 0x0000_0000_0001).unwrap();
    let mut message = payload.to_message();
    assert_eq!(Payload::from_message(&message), Some(payload));
    message[9] ^= 1;
    assert_eq!(Payload::from_message(&message), None);
    let mut truncated = payload.to_message();
    truncated.pop();
    assert_eq!(Payload::from_message(&truncated), None);
}

#[test]
fn payload_fields_round_trip_and_reject_overflow() {
    let payload = Payload::new(0xF, 0xF, 0xFFFF_FFFF_FFFF).unwrap();
    assert_eq!(Payload::from_bytes(&payload.to_bytes()).unwrap(), payload);
    assert!(Payload::new(0x10, 0, 0).is_err());
    assert!(Payload::new(0, 0x10, 0).is_err());
    assert!(Payload::new(0, 0, 1 << 48).is_err());
}

/// Acoustic re-recording is the literal string, never a null and never absent. Null reads as
/// "not yet measured" and invites hope; there is no measurement pending.
#[test]
fn capabilities_state_acoustic_rerecording_as_unsupported() {
    use apw_watermark::{ACOUSTIC_RERECORDING, Capabilities};
    assert_eq!(ACOUSTIC_RERECORDING, "unsupported");
    for rate in [32_000u32, 44_100, 48_000, 96_000] {
        let capabilities = Capabilities::at(rate);
        assert_eq!(capabilities.acoustic_rerecording, "unsupported");
        assert_eq!(capabilities.time_stretch, "unsupported");
        assert!(capabilities.measured_rate_range <= capabilities.searched_rate_range);
        assert_eq!(capabilities.measured_transparency, None);
        assert_eq!(capabilities.measured_survival, None);
        assert_eq!(capabilities.payload_bits, 56);
        assert!(capabilities.guaranteed_seconds > capabilities.block_seconds);
    }
    let at_44100 = Capabilities::at(44_100);
    assert!((at_44100.block_seconds - 9.66).abs() < 0.01);
    assert!((at_44100.guaranteed_seconds - 19.32).abs() < 0.02);
}
