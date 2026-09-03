#![allow(clippy::unwrap_used, clippy::panic)]

use std::f64::consts::TAU;

use audio_provenance_audio::biquad::{self, BiquadCoefficients};
use audio_provenance_audio::buffer::AudioBuffer;
use audio_provenance_audio::metrics::{Spectrum, rms};
use audio_provenance_audio::resample::{ClockDrift, resample};

fn sine(rate: u32, frequency: f64, frames: usize) -> AudioBuffer {
    let planes: Vec<f32> = (0..frames)
        .map(|frame| (TAU * frequency * frame as f64 / f64::from(rate)).sin() as f32)
        .collect();
    AudioBuffer::from_planes(rate, 1, planes).unwrap()
}

/// Peak bin of a 4096-point analysis, in Hz.
fn dominant_hz(samples: &[f32], rate: u32) -> f64 {
    let spectrum = Spectrum::analyze(&samples[..4096], rate).unwrap();
    let magnitudes = spectrum.magnitudes();
    let peak = (1..magnitudes.len() / 2)
        .max_by(|a, b| magnitudes[*a].total_cmp(&magnitudes[*b]))
        .unwrap();
    peak as f64 * spectrum.bin_hz()
}

/// A round trip through a non-integer ratio is where a resampler shows both of
/// its common defects at once: a shifted pitch, and a gain that creeps because
/// the filter is not unity at DC.
#[test]
fn a_1khz_sine_survives_48k_to_44k1_and_back() {
    let source = sine(48_000, 1_000.0, 48_000);
    let down = resample(&source, 44_100).unwrap();
    let up = resample(&down, 48_000).unwrap();

    assert_eq!(down.sample_rate(), 44_100);
    assert_eq!(up.sample_rate(), 48_000);
    let ideal = 44_100usize;
    assert!(
        down.frames().abs_diff(ideal) < 64,
        "downsampled to {} frames, expected about {ideal}",
        down.frames()
    );

    for (label, buffer) in [("downsampled", &down), ("round tripped", &up)] {
        let found = dominant_hz(&buffer.channel(0).unwrap()[8_000..], buffer.sample_rate());
        assert!(
            (found - 1_000.0).abs() < 15.0,
            "{label} dominant frequency {found} Hz"
        );
    }

    // Compare interiors so the resampler's edge transients do not stand in for a
    // gain error.
    let source_rms = rms(&source.channel(0).unwrap()[8_000..40_000]);
    let round_trip_rms = rms(&up.channel(0).unwrap()[8_000..40_000]);
    let ratio = round_trip_rms / source_rms;
    assert!(
        (ratio - 1.0).abs() < 0.01,
        "round trip changed level by a factor of {ratio}"
    );
}

#[test]
fn clock_drift_stretches_the_content_without_moving_the_sample_rate() {
    let source = sine(48_000, 1_000.0, 48_000);
    let drifted = ClockDrift::new(1_000.0).unwrap().apply(&source).unwrap();

    assert_eq!(drifted.sample_rate(), 48_000);
    assert_eq!(drifted.frames(), 48_048);

    // 1000 ppm fast means the same waveform now occupies 1.001x the samples, so
    // the tone reads 0.1% lower against an unchanged clock.
    let found = dominant_hz(&drifted.channel(0).unwrap()[8_000..], 48_000);
    assert!(
        (found - 999.0).abs() < 15.0,
        "drifted dominant frequency {found} Hz"
    );

    assert!(ClockDrift::new(f64::NAN).is_err());
    assert!(ClockDrift::new(1.0e9).is_err());
}

#[test]
fn biquads_pass_and_reject_the_bands_they_are_built_for() {
    let low = sine(48_000, 200.0, 24_000);
    let high = sine(48_000, 12_000.0, 24_000);

    let low_pass = BiquadCoefficients::low_pass(48_000, 1_000.0, 0.707).unwrap();
    let high_pass = BiquadCoefficients::high_pass(48_000, 1_000.0, 0.707).unwrap();

    for (coefficients, passed, rejected) in [(low_pass, &low, &high), (high_pass, &high, &low)] {
        let mut kept = passed.clone();
        biquad::apply(&mut kept, coefficients);
        let mut cut = rejected.clone();
        biquad::apply(&mut cut, coefficients);

        let kept_ratio =
            rms(&kept.channel(0).unwrap()[4_000..]) / rms(&passed.channel(0).unwrap()[4_000..]);
        let cut_ratio =
            rms(&cut.channel(0).unwrap()[4_000..]) / rms(&rejected.channel(0).unwrap()[4_000..]);
        assert!(kept_ratio > 0.95, "passband attenuated to {kept_ratio}");
        assert!(cut_ratio < 0.05, "stopband only reached {cut_ratio}");
    }

    let peaking = BiquadCoefficients::peaking(48_000, 200.0, 1.0, 12.0).unwrap();
    let mut boosted = low.clone();
    biquad::apply(&mut boosted, peaking);
    let gain = rms(&boosted.channel(0).unwrap()[4_000..]) / rms(&low.channel(0).unwrap()[4_000..]);
    assert!(
        (20.0 * gain.log10() - 12.0).abs() < 0.5,
        "peaking filter gave {} dB",
        20.0 * gain.log10()
    );

    assert!(BiquadCoefficients::low_pass(48_000, 24_000.0, 0.707).is_err());
    assert!(BiquadCoefficients::low_pass(48_000, 1_000.0, 0.0).is_err());
    assert!(BiquadCoefficients::peaking(48_000, 1_000.0, 1.0, f64::INFINITY).is_err());
}
