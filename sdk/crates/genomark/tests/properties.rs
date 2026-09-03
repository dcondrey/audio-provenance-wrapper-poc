#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The five properties GenoMark was chosen for. Run with `--release`: every case here drives the
//! real STFT over tens of seconds of audio, which is minutes per case in an unoptimised build.

mod support;

use audio_provenance_bench::perceptual;
use genomark::params::MAX_CRC_ATTEMPTS;
use genomark::{ConfidenceClass, DELTA, GenoMark, Payload};

const SECONDS: f64 = 25.0;
const RATE: u32 = 48_000;

fn payload() -> Payload {
    Payload::new(1, 0, 0x9AC3_5E00_11A7).expect("payload fields are in range")
}

/// Property 1. A scalar gain adds the same constant to both cells' log energies, so it cancels in
/// the statistic with no memory. This is the decisive advantage over a normalised design, so it is
/// pinned as bit-identity of the recovered payload rather than as a tolerance.
#[test]
fn gain_invariance_recovers_the_identical_payload() {
    let audio = support::tone_bed(RATE, SECONDS, 2);
    let mark = GenoMark::public();
    let expected = payload();
    let marked = mark.embed(&audio, expected).unwrap();
    let reference = mark.detect(&marked).unwrap();
    assert_eq!(reference.payload(), Some(expected));

    // Powers of two scale the float exponent alone, so the spectra are bit-for-bit `g * X`.
    for gain in [0.25f32, 0.5, 2.0, 4.0] {
        let outcome = mark.detect(&support::scaled(&marked, gain)).unwrap();
        assert_eq!(
            outcome.payload(),
            Some(expected),
            "payload changed under an exact power-of-two gain of {gain}"
        );
        assert_eq!(outcome.class(), reference.class());
    }
    // Non-powers of two round differently in every bin and still land on the same payload.
    for gain in [0.37f32, 1.7, 11.0] {
        let outcome = mark.detect(&support::scaled(&marked, gain)).unwrap();
        assert_eq!(
            outcome.payload(),
            Some(expected),
            "payload changed under a gain of {gain}"
        );
    }
}

/// Property 5, and property 2's positive half: the detector is handed audio and nothing else, and
/// two different payloads in the same host recover as themselves. A detector that returned a
/// constant, or that had been told what to look for, passes neither half.
#[test]
fn round_trip_recovers_what_was_embedded_and_only_that() {
    let audio = support::tone_bed(RATE, SECONDS, 2);
    let mark = GenoMark::public();
    let first = payload();
    let second = Payload::new(1, 0, 0x0123_4567_89AB).unwrap();

    let one = mark.detect(&mark.embed(&audio, first).unwrap()).unwrap();
    let two = mark.detect(&mark.embed(&audio, second).unwrap()).unwrap();
    assert_eq!(one.payload(), Some(first));
    assert_eq!(two.payload(), Some(second));
    assert_eq!(one.class(), ConfidenceClass::Strong);
    assert_eq!(one.bits_corrected(), 0);
}

/// Property 2. The lattice, the preamble and the pilots are keyed, so a detector holding a
/// different profile key has nothing to correlate against and reports nothing. There is no code
/// path that can be handed the payload to look for.
#[test]
fn a_detector_without_the_profile_key_reports_nothing() {
    let audio = support::tone_bed(RATE, SECONDS, 2);
    let embedder = GenoMark::public();
    let marked = embedder.embed(&audio, payload()).unwrap();

    let stranger = GenoMark::keyed(b"a different profile key entirely".to_vec(), 3).unwrap();
    let outcome = stranger.detect(&marked).unwrap();
    assert_eq!(outcome.payload(), None);
    assert_eq!(outcome.class(), ConfidenceClass::None);
}

/// Property 3. Acceptance is CRC-32C or nothing. Over audio that was never marked the detector may
/// examine candidates, but it may not return a payload.
#[test]
fn never_marked_audio_returns_no_payload() {
    let mark = GenoMark::public();
    let hosts = [
        support::tone_bed(RATE, SECONDS, 2),
        support::tone_bed(RATE, SECONDS, 1),
        support::tone_bed(44_100, SECONDS, 2),
        support::pure_tone(RATE, SECONDS, 1000.0),
        support::white_noise(RATE, SECONDS),
    ];
    for (index, host) in hosts.iter().enumerate() {
        let outcome = mark.detect(host).unwrap();
        assert_eq!(
            outcome.payload(),
            None,
            "host {index} produced a false accept after {} crc attempts",
            outcome.crc_attempts()
        );
    }
}

/// Property 4. The spec fixes the embedding budget as a per-bin amplitude change: typical
/// +-0.43 dB, worst case +-0.87 dB, from a lattice shift bounded by DELTA/2.
///
/// It fixes no segmental-SNR or noise-to-mask threshold; its inaudibility gate is PEAQ ODG plus a
/// blinded panel, neither of which has run. So the shift bound and the energy-weighted per-bin
/// change are asserted, and the model measures are recorded.
#[test]
fn embedding_stays_inside_the_spec_budget() {
    let audio = support::tone_bed(RATE, SECONDS, 2);
    let mark = GenoMark::public();
    let (marked, report) = mark.embed_measured(&audio, payload()).unwrap();

    assert!(
        report.max_intended_shift_nepers <= DELTA / 2.0 + 1e-9,
        "lattice shift {} exceeds DELTA/2",
        report.max_intended_shift_nepers
    );
    let gain = report.bin_gain.expect("closure was measured");
    assert!(
        gain.energy_weighted_rms_db <= 0.87,
        "energy-weighted per-bin change {:.3} dB exceeds the spec's worst-case budget",
        gain.energy_weighted_rms_db
    );
    assert!(
        report.meets_closure_budget(),
        "closure residual {:?} exceeds DELTA/20",
        report.closure_residual_nepers
    );

    let measured = perceptual::measure(&audio, &marked).unwrap();
    let segmental = measured.segmental_snr_db.expect("segments were scored");
    let mask = measured.noise_to_mask_max_db.expect("frames were scored");
    assert!(segmental > 30.0, "segmental SNR {segmental:.1} dB");
    assert!(
        mask < 0.0,
        "worst noise-to-mask ratio {mask:.1} dB is above the mask"
    );
    assert_eq!(measured.frames_above_mask_fraction, Some(0.0));
}

/// Under one block of contiguous audio there is nothing to decode, and the answer is nothing rather
/// than a partial payload. Boundary at the block length, not at a round number of seconds.
#[test]
fn short_audio_returns_nothing_rather_than_a_guess() {
    let mark = GenoMark::public();
    let capabilities = mark.capabilities(RATE);
    let under = support::tone_bed(RATE, capabilities.block_seconds - 0.5, 2);
    assert!(mark.embed(&under, payload()).is_err());
    let outcome = mark.detect(&under).unwrap();
    assert_eq!(outcome.payload(), None);
    assert_eq!(outcome.class(), ConfidenceClass::None);
}

/// The section 9 false-positive arithmetic is only true while the search is bounded. If a host
/// drives the retained candidate set to its cap, peaks above tau_sync are dropped unexamined and
/// that host's recovery number stops being a measurement of the channel.
#[test]
fn the_candidate_search_stays_far_inside_its_cap() {
    let mark = GenoMark::public();
    let marked = mark
        .embed(&support::tone_bed(RATE, SECONDS, 2), payload())
        .unwrap();
    let hosts = [
        marked.clone(),
        support::scaled(&marked, 0.1),
        support::white_noise(RATE, SECONDS),
        support::pure_tone(RATE, SECONDS, 2000.0),
        support::tone_bed(RATE, SECONDS, 1),
    ];
    for (index, host) in hosts.iter().enumerate() {
        let outcome = mark.detect(host).unwrap();
        assert!(
            !outcome.candidate_cap_reached(),
            "host {index} saturated the candidate cap at {} crc attempts",
            outcome.crc_attempts()
        );
        assert!(outcome.crc_attempts() <= MAX_CRC_ATTEMPTS);
    }
}

/// A free-running playback clock is what the rate grid exists for.
///
/// The spec predicts +-6%; measured, this recovers to about +-0.3% and is gone by +-0.5%. Both
/// signs are driven, because the failures this pins were all one-sided: rounding each cell boundary
/// to an integer bin made the grid inert below the true rate, and suppressing candidates by offset
/// alone let a coarse hypothesis one step off the truth capture the block and carry a fine grid that
/// could not reach it.
///
/// IMPORTANT: the band-limited resampler arm is not decoration. The `ClockDrift` arm alone passed
/// throughout the period the bench measured +-0.1% drift at 0.400 exact recovery, because a cubic
/// interpolator over a synthetic bed is a gentler channel than a sinc resampler over real music.
/// A rate test that does not drive the resampler does not pin the rate search.
#[test]
fn playback_rate_drift_decodes_across_the_searched_range() {
    let audio = support::tone_bed(RATE, SECONDS, 2);
    let mark = GenoMark::public();
    let expected = payload();
    let marked = mark.embed(&audio, expected).unwrap();
    let mut failures: Vec<String> = Vec::new();
    for ppm in [-3_000.0f64, -1_000.0, -100.0, 100.0, 1_000.0, 3_000.0] {
        let drifted = audio_provenance_audio::ClockDrift::new(ppm)
            .unwrap()
            .apply(&marked)
            .unwrap();
        let outcome = mark.detect(&drifted).unwrap();
        if outcome.payload() != Some(expected) {
            failures.push(format!("clock {ppm} ppm"));
        }
    }
    for factor in [0.999f64, 1.001] {
        let drifted = audio_provenance_audio::resample_ratio(&marked, 1.0 / factor).unwrap();
        let outcome = mark.detect(&drifted).unwrap();
        if outcome.payload() != Some(expected) {
            failures.push(format!("playback rate {factor}"));
        }
    }
    assert!(
        failures.is_empty(),
        "drift that lost the payload: {failures:?}"
    );
}

/// Cross-block combining sums soft evidence, so it must not become a second way in.
///
/// The accumulator integrates over every block a grid reaches, and the grid search that feeds it
/// promotes lattices no single preamble peak would have promoted. Over audio that was never marked
/// that is the whole false-positive surface of the change, and the answer has to stay nothing. The
/// host is long enough to hold several blocks, because a one-block host cannot exercise the path at
/// all.
#[test]
fn cross_block_combining_never_accepts_unmarked_audio() {
    let mark = GenoMark::public();
    let long = SECONDS * 4.0;
    let hosts = [
        support::tone_bed(RATE, long, 2),
        support::tone_bed(RATE, long, 1),
        support::white_noise(RATE, long),
        support::pure_tone(RATE, long, 1000.0),
    ];
    for (index, host) in hosts.iter().enumerate() {
        let outcome = mark.detect(host).unwrap();
        assert_eq!(
            outcome.payload(),
            None,
            "host {index} produced a false accept over {} combined blocks after {} crc attempts",
            outcome.blocks_combined(),
            outcome.crc_attempts()
        );
        assert_eq!(outcome.blocks_combined(), 0);
    }
}

/// Combining reports itself as weaker than independent per-block agreement, and never inflates the
/// per-block count the coverage guard divides by.
///
/// `blocks_accepted` is what `genotrace` turns into block coverage, and what fails a marked insert
/// spliced into unmarked material. A payload that only the accumulator could reach must leave that
/// count at zero and stay out of the `Strong` class.
#[test]
fn a_combined_only_detection_is_reported_as_weaker_than_a_per_block_one() {
    let mark = GenoMark::public();
    let marked = mark
        .embed(&support::tone_bed(RATE, SECONDS * 4.0, 2), payload())
        .unwrap();
    let clean = mark.detect(&marked).unwrap();
    assert_eq!(clean.payload(), Some(payload()));
    assert_eq!(clean.class(), ConfidenceClass::Strong);
    assert_eq!(clean.blocks_combined(), 0);

    // Measured on this bed: 24 dB still leaves four blocks passing the CRC on their own, and by
    // 22 dB none do while ten still combine into the right payload. That gap is the whole point of
    // the accumulator, and it is where this invariant has to hold.
    let mut worst: Option<genomark::DetectionOutcome> = None;
    for snr_db in [22.0f64, 20.0] {
        let outcome = mark.detect(&support::noised(&marked, snr_db)).unwrap();
        if outcome.payload() == Some(payload()) && outcome.blocks_combined() > 0 {
            worst = Some(outcome);
        }
    }
    let combined = worst.expect("no noise level put the accumulator on the answering path");
    assert_eq!(combined.blocks_accepted(), 0);
    assert_eq!(combined.class(), ConfidenceClass::Single);
    assert!(combined.blocks_combined() >= 2);
}
