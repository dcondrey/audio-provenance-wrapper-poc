#![allow(clippy::unwrap_used, clippy::panic)]

use audio_provenance_audio::mdct::{Mdct, MdctTransform};
use audio_provenance_audio::stft::Stft;
use audio_provenance_audio::window::{Symmetry, blackman_harris, hann, princen_bradley};

fn noise(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            ((state >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        })
        .collect()
}

fn max_abs_diff(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .fold(0.0f32, |acc, (a, b)| acc.max((a - b).abs()))
}

/// The WOLA normaliser divides by the summed squared window, so it reconstructs
/// whatever framing it is given. This asserts the framing itself is right: with
/// periodic Hann at 75% overlap the raw denominator must be the flat 1.5 that
/// squared-COLA predicts across the whole signal region.
#[test]
fn periodic_hann_at_quarter_hop_gives_a_flat_cola_denominator() {
    let stft = Stft::new(hann(1024, Symmetry::Periodic), 256).unwrap();
    let signal_len = 5000;
    let sum = stft.cola_sum(signal_len);
    let interior = &sum[stft.pad()..stft.pad() + signal_len];
    for value in interior {
        assert!(
            (value - 1.5).abs() < 1.0e-5,
            "cola denominator {value} is not the flat 1.5 expected at hop = window / 4"
        );
    }
}

#[test]
fn stft_round_trips_within_f32_tolerance() {
    let signal = noise(9_001, 0x5eed_0001);
    for window in [
        hann(1024, Symmetry::Periodic),
        blackman_harris(1024, Symmetry::Periodic),
    ] {
        let stft = Stft::new(window, 256).unwrap();
        let frames = stft.forward(&signal).unwrap();
        let restored = stft.inverse(&frames, signal.len()).unwrap();
        assert_eq!(restored.len(), signal.len());
        let error = max_abs_diff(&signal, &restored);
        assert!(error < 1.0e-4, "stft round trip error {error} is too large");
    }
}

/// A single-block MDCT round trip cancels its own aliasing and so passes even
/// with the wrong phase offset. Reconstructing only the middle half of three
/// half-overlapping blocks is what actually exercises TDAC.
#[test]
fn mdct_cancels_time_domain_aliasing_across_three_blocks() {
    const HALF: usize = 256;
    let signal = noise(HALF * 4, 0x5eed_0002);
    let window = princen_bradley(HALF);
    let mut mdct = Mdct::new(HALF).unwrap();

    let mut overlapped = vec![0.0f32; HALF * 4];
    let mut coefficients = vec![0.0f32; HALF];
    let mut block = vec![0.0f32; HALF * 2];
    for index in 0..3 {
        let base = index * HALF;
        for (offset, slot) in block.iter_mut().enumerate() {
            *slot = signal[base + offset] * window[offset];
        }
        mdct.forward(&block, &mut coefficients).unwrap();
        mdct.inverse(&coefficients, &mut block).unwrap();
        for (offset, value) in block.iter().enumerate() {
            overlapped[base + offset] += value * window[offset];
        }
    }

    let error = max_abs_diff(&signal[HALF..HALF * 3], &overlapped[HALF..HALF * 3]);
    assert!(
        error < 1.0e-5,
        "tdac reconstruction error {error} is too large"
    );
}

/// Pins the `2 / M` synthesis scale. A relative-shape check would pass with the
/// wrong constant; a constant input must come back at its own amplitude.
#[test]
fn mdct_preserves_amplitude() {
    const HALF: usize = 64;
    let signal = vec![0.75f32; HALF * 4];
    let window = princen_bradley(HALF);
    let mut mdct = Mdct::new(HALF).unwrap();

    let mut overlapped = vec![0.0f32; HALF * 4];
    let mut coefficients = vec![0.0f32; HALF];
    let mut block = vec![0.0f32; HALF * 2];
    for index in 0..3 {
        let base = index * HALF;
        for (offset, slot) in block.iter_mut().enumerate() {
            *slot = signal[base + offset] * window[offset];
        }
        mdct.forward(&block, &mut coefficients).unwrap();
        mdct.inverse(&coefficients, &mut block).unwrap();
        for (offset, value) in block.iter().enumerate() {
            overlapped[base + offset] += value * window[offset];
        }
    }

    for value in &overlapped[HALF..HALF * 2] {
        assert!(
            (value - 0.75).abs() < 1.0e-5,
            "reconstructed amplitude {value} is not the input amplitude 0.75"
        );
    }
}

#[test]
fn mdct_transform_round_trips_a_whole_signal() {
    let signal = noise(7_777, 0x5eed_0003);
    let mut transform = MdctTransform::new(512).unwrap();
    let coefficients = transform.analyze(&signal).unwrap();
    let restored = transform.synthesize(&coefficients, signal.len()).unwrap();
    let error = max_abs_diff(&signal, &restored);
    assert!(error < 1.0e-5, "mdct round trip error {error} is too large");
}

#[test]
fn princen_bradley_window_satisfies_its_condition() {
    const HALF: usize = 128;
    let window = princen_bradley(HALF);
    for n in 0..HALF {
        let sum = window[n] * window[n] + window[n + HALF] * window[n + HALF];
        assert!((sum - 1.0).abs() < 1.0e-6, "w[n]^2 + w[n+M]^2 == {sum}");
    }
}
