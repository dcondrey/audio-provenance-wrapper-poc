#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use audio_provenance_audio::AudioBuffer;
use audio_provenance_audio::convolve::convolve;
use audio_provenance_audio::fft::RealFft;
use audio_provenance_bench::audio::{decode_wav, encode_wav_f32, from_interleaved};
use audio_provenance_bench::channel::Channel;
use audio_provenance_bench::channel::acoustic::{AcousticRerecord, RoomPreset};
use audio_provenance_bench::channel::codec::{LossyCodec, LossyFormat};
use audio_provenance_bench::channel::native::{Crop, Gain, Identity, Requantize, ResampleVia};
use audio_provenance_bench::corpus::{ContentClass, CorpusItem, CorpusSource, CorpusSpec};
use audio_provenance_bench::dsp::rng::Rng;
use audio_provenance_bench::fixtures::{AlwaysAcceptFixture, Lsb16Fixture};
use audio_provenance_bench::ports::{CommandRunner, UnavailableRunner};
use audio_provenance_bench::report::{RowVerdict, Thresholds};
use audio_provenance_bench::runner::{BenchConfig, run};
use audio_provenance_bench::watermark::WatermarkCodec;
use audio_provenance_core::CodedError;

const RATE: u32 = 48_000;

fn tone(frequency: f64, seconds: f64) -> AudioBuffer {
    let frames = (seconds * f64::from(RATE)) as usize;
    let samples: Vec<f32> = (0..frames)
        .flat_map(|n| {
            let t = n as f64 / f64::from(RATE);
            let value = (0.4 * (core::f64::consts::TAU * frequency * t).sin()) as f32;
            [value, value * 0.9]
        })
        .collect();
    from_interleaved(RATE, 2, &samples).unwrap()
}

fn item(id: &str, audio: AudioBuffer) -> CorpusItem {
    CorpusItem::new(
        id,
        ContentClass::HarmonicPad,
        CorpusSource::Synthetic {
            generator: "test".to_owned(),
            recipe: "tone".to_owned(),
        },
        audio,
    )
    .unwrap()
}

/// The 7.3 s crop exists because it lands off every common analysis-frame grid; a later refactor
/// that rounds it to a tidy offset would remove the only channel that breaks naive synchronisation.
#[test]
fn crop_7s3_lands_off_every_common_frame_grid() {
    let crop = Crop::new("crop_7s3", 7.3);
    assert_eq!(
        RATE,
        CorpusSpec::default().sample_rate,
        "the off-grid property is a property of the crop AT THE WORKING RATE, so the test must use it"
    );
    let dropped = crop.dropped_frames(RATE);
    assert_eq!(dropped, 350_400);
    for frame_size in [512usize, 1024, 2048, 4096, 1152] {
        assert_ne!(
            dropped % frame_size,
            0,
            "7.3 s at {RATE} Hz is a whole number of {frame_size}-sample frames"
        );
    }
    let cropped = crop
        .apply(&tone(440.0, 9.0), 1, &UnavailableRunner)
        .unwrap();
    assert_eq!(cropped.frames(), 9 * RATE as usize - dropped);
}

/// The corpus loader reads files the bench did not write, so the reader is the crate's untrusted
/// boundary. A declared size larger than the file is refused outright rather than clamped: a
/// truncated file silently shortened to whatever survived would enter the corpus as a measurement
/// of content that is not there.
#[test]
fn wav_reader_refuses_hostile_headers() {
    let audio = tone(440.0, 0.1);
    let good = encode_wav_f32(&audio).unwrap();
    assert_eq!(decode_wav(&good).unwrap().frames(), audio.frames());

    assert!(decode_wav(b"RIFFxxxxWAVE").is_err());
    assert!(decode_wav(&good[..8]).is_err());

    let payload_bytes = audio.frames() * audio.channels() * 4;
    let mut oversized = good.clone();
    let data_size_at = oversized.len() - payload_bytes - 4;
    oversized[data_size_at..data_size_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let refused = decode_wav(&oversized).unwrap_err();
    assert_eq!(
        refused.code(),
        "audio_chunk_truncated",
        "a data chunk claiming more bytes than the file holds must be refused, never read past the \
         end: got {refused}"
    );

    let mut no_fmt = good.clone();
    no_fmt[12..16].copy_from_slice(b"junk");
    assert!(decode_wav(&no_fmt).is_err());
}

/// The bench's whole claim is that it separates a mark that survives a path from one that does not.
/// The fixture is bit-exact through identity and gone the moment sample values move.
#[test]
fn lsb16_fixture_survives_identity_and_dies_under_requantisation() {
    let codec = Lsb16Fixture::new(8);
    let payload = vec![0x47, 0x54, 0x01, 0x9A, 0xC3, 0x5E, 0x00, 0x11];
    let marked = codec.embed(&tone(440.0, 2.0), &payload).unwrap();

    let clean = Identity.apply(&marked, 7, &UnavailableRunner).unwrap();
    assert_eq!(
        codec.detect(&clean).unwrap().payload.as_deref(),
        Some(&payload[..])
    );

    let crushed = Requantize::new("requantize_8bit", 8)
        .apply(&marked, 7, &UnavailableRunner)
        .unwrap();
    assert_ne!(
        codec.detect(&crushed).unwrap().payload.as_deref(),
        Some(&payload[..]),
        "an 8-bit requantisation destroys the 16-bit LSB the fixture rides in"
    );

    let resampled = ResampleVia::new("resample_via_22050", 22_050)
        .apply(&marked, 7, &UnavailableRunner)
        .unwrap();
    assert_ne!(
        codec.detect(&resampled).unwrap().payload.as_deref(),
        Some(&payload[..])
    );
}

/// The harness self-test that `docs/BUILD_ORDER.md` unit B0 requires: a bench that cannot fail a
/// known-bad detector has been executed, not validated.
#[test]
fn false_positive_arm_fires_against_a_detector_that_always_accepts() {
    let payload = vec![0x47, 0x54, 0x01, 0x9A, 0xC3, 0x5E, 0x00, 0x11];
    let codec = AlwaysAcceptFixture::new(payload.clone());
    let corpus = vec![
        item("tone_a", tone(440.0, 1.0)),
        item("tone_b", tone(311.0, 1.0)),
    ];
    let matrix: Vec<Box<dyn Channel>> = vec![
        Box::new(Identity),
        Box::new(Requantize::new("requantize_16bit", 16)),
    ];
    let config = BenchConfig {
        payload,
        thresholds: Thresholds::uniform_minimum(0.99),
        measure_perceptual: false,
        ..BenchConfig::default()
    };
    let report = run(&codec, &corpus, &matrix, &UnavailableRunner, &config).unwrap();

    assert_eq!(report.totals.false_positive_accepts, 4);
    assert_eq!(report.totals.overall_false_positive_rate, Some(1.0));
    assert_eq!(report.verdict, RowVerdict::Fail);
    for row in &report.rows {
        assert_eq!(row.exact_recovery_rate, Some(1.0));
        assert_eq!(
            row.verdict,
            RowVerdict::Fail,
            "a perfect recovery rate must not pass while every unmarked trial also accepts"
        );
    }
}

/// A channel whose external program is missing has to appear as an explicit error row and fail the
/// run. Dropping it would turn an unmeasured path into a silently clean matrix.
#[test]
fn a_channel_with_no_available_program_becomes_an_error_row() {
    let codec = Lsb16Fixture::new(8);
    let corpus = vec![item("tone_a", tone(440.0, 1.0))];
    let matrix: Vec<Box<dyn Channel>> = vec![
        Box::new(Identity),
        Box::new(LossyCodec::new(
            "mp3_128",
            LossyFormat::Mp3,
            128,
            "/nonexistent/ffmpeg",
        )),
    ];
    let config = BenchConfig {
        measure_perceptual: false,
        ..BenchConfig::default()
    };
    let report = run(&codec, &corpus, &matrix, &UnavailableRunner, &config).unwrap();

    assert_eq!(report.channels_expected, 2);
    assert_eq!(report.channels_reported, 2);
    assert!(!report.truncated);
    let mp3 = report
        .rows
        .iter()
        .find(|row| row.channel == "mp3_128")
        .expect("the unrunnable channel must still have a row");
    assert_eq!(mp3.verdict, RowVerdict::Error);
    assert_eq!(
        mp3.error.as_ref().map(|e| e.code.as_str()),
        Some("port_unavailable")
    );
    assert_eq!(report.verdict, RowVerdict::Fail);
    assert!(report.to_table().contains("mp3_128"));
}

/// The acoustic rows depend on FFT convolution for tractability, so the kernel is pinned against a
/// naive reference rather than against itself.
#[test]
fn fft_and_overlap_add_convolution_match_naive_references() {
    let fft = RealFft::<f64>::new(64).unwrap();
    let mut rng = Rng::new(9);
    let signal: Vec<f64> = (0..64).map(|_| f64::from(rng.next_symmetric())).collect();
    let spectrum = fft.power_spectrum(&signal).unwrap();
    assert_eq!(
        spectrum.len(),
        33,
        "a real transform reports the non-redundant half"
    );
    for (k, power) in spectrum.iter().enumerate() {
        let mut re = 0.0;
        let mut im = 0.0;
        for (n, &x) in signal.iter().enumerate() {
            let angle = -2.0 * core::f64::consts::PI * k as f64 * n as f64 / 64.0;
            re += x * angle.cos();
            im += x * angle.sin();
        }
        assert!(
            (power - (re * re + im * im)).abs() < 1e-6,
            "bin {k}: fft {power} vs dft {}",
            re * re + im * im
        );
    }

    let x: Vec<f32> = (0..1024).map(|_| rng.next_symmetric() * 0.3).collect();
    let h: Vec<f32> = (0..200)
        .map(|n| rng.next_symmetric() * (-(n as f32) / 60.0).exp())
        .collect();
    let fast = convolve(&x, &h).unwrap();
    assert_eq!(fast.len(), x.len() + h.len() - 1);
    for i in [0usize, 1, 199, 200, 511, 1023, 1222] {
        let mut expected = 0.0f64;
        for (j, &tap) in h.iter().enumerate() {
            if j <= i
                && let Some(&sample) = x.get(i - j)
            {
                expected += f64::from(sample) * f64::from(tap);
            }
        }
        assert!(
            (f64::from(fast[i]) - expected).abs() < 1e-4,
            "tap {i}: overlap-add {} vs direct {expected}",
            fast[i]
        );
    }
}

/// The resampler carries the sample-rate and clock-drift rows. A tone through 48k -> 44.1k -> 48k
/// has to come back recognisably, or those rows measure the resampler rather than the watermark.
///
/// Delay and distortion are separated deliberately. A polyphase resampler has a group delay, and
/// this one leaves about 1.1 samples of it after rubato trims the integer part, so a raw
/// sample-by-sample SNR reads 16 dB at 1 kHz and 3 dB at 5 kHz purely from the shift. That is a
/// time offset, not damage: the bench reports it as `mean_frames_delta` and never realigns before
/// detection, because sync is the detector's job. So the fidelity assertion compensates the delay
/// and the delay gets its own bound.
#[test]
fn resampling_round_trip_preserves_a_tone() {
    const HZ: f64 = 1_000.0;
    let source = tone(HZ, 1.0);
    let round_tripped = ResampleVia::new("resample_via_44100", 44_100)
        .apply(&source, 3, &UnavailableRunner)
        .unwrap();
    let got = round_tripped.mono_sum();

    // Amplitude comes from the source rather than a literal, so the probe stays correct if the
    // corpus helper's level or channel weighting changes.
    let reference = source.mono_sum();
    let mean_square: f64 = reference
        .iter()
        .map(|&s| f64::from(s) * f64::from(s))
        .sum::<f64>()
        / reference.len() as f64;
    let amplitude = (2.0 * mean_square).sqrt();

    // The probe is an analytic tone, so the reference at any fractional shift is exact and the
    // search needs no interpolation of its own.
    let snr_at = |tau: f64| -> f64 {
        let (mut signal, mut error) = (0.0f64, 0.0f64);
        let interior = got.len().saturating_sub(4_000);
        for (i, &sample) in got.iter().enumerate().take(interior).skip(4_000) {
            let ideal = amplitude
                * (core::f64::consts::TAU * HZ * (i as f64 - tau) / f64::from(RATE)).sin();
            let observed = f64::from(sample);
            signal += ideal * ideal;
            error += (ideal - observed) * (ideal - observed);
        }
        10.0 * (signal / error.max(1e-30)).log10()
    };

    let mut best = (0.0f64, f64::NEG_INFINITY);
    let mut tau = -4.0f64;
    while tau <= 4.0 {
        let snr = snr_at(tau);
        if snr > best.1 {
            best = (tau, snr);
        }
        tau += 0.002;
    }
    let (delay, snr_db) = best;

    assert!(
        delay.abs() < 2.0,
        "the channel slid the audio by {delay:.2} samples; a resample row must not become an \
         arbitrary time shift"
    );
    assert!(
        snr_db > 60.0,
        "round-trip SNR was only {snr_db:.1} dB once the {delay:.2} sample group delay is removed, \
         so the resampler itself is destroying the signal"
    );
}

#[test]
fn unavailable_runner_reports_no_program_as_available() {
    assert!(!UnavailableRunner.is_available("/opt/homebrew/bin/ffmpeg"));
}

/// The masking measure only means anything if band energies and the threshold in quiet share units.
/// They did not at first: the floor was in per-sample power while the bands were raw DFT power, so
/// the floor never bound and a residual 100 dB down still scored as audible.
#[test]
fn masking_measure_separates_an_inaudible_residual_from_an_obvious_one() {
    let reference = tone(1_000.0, 1.0);
    let mut rng = Rng::new(4);
    let noisy = |sigma: f32, rng: &mut Rng| {
        let samples: Vec<f32> = reference
            .to_interleaved()
            .into_iter()
            .map(|s| rng.next_gaussian().mul_add(sigma, s))
            .collect();
        from_interleaved(RATE, 2, &samples).unwrap()
    };

    let quiet =
        audio_provenance_bench::perceptual::measure(&reference, &noisy(1e-5, &mut rng)).unwrap();
    let loud =
        audio_provenance_bench::perceptual::measure(&reference, &noisy(3e-2, &mut rng)).unwrap();

    let quiet_nmr = quiet.noise_to_mask_mean_db.unwrap();
    let loud_nmr = loud.noise_to_mask_mean_db.unwrap();
    assert!(
        quiet_nmr < 0.0,
        "a -100 dBFS residual under a full-scale tone must sit below the masking threshold, got {quiet_nmr:.1} dB"
    );
    assert!(
        loud_nmr > 20.0,
        "a -30 dBFS residual must sit far above it, got {loud_nmr:.1} dB"
    );
    assert!(quiet.segmental_snr_db.unwrap() > loud.segmental_snr_db.unwrap());
}

/// The acoustic rows are only worth reading if the convolution is real. A radix-2 bit-reversal bug
/// made `convolve` return noise for any impulse response over 64 taps, which is every room preset,
/// and the acoustic rows still read 0.000 exact recovery: indistinguishable from reverb working. So
/// the reverberation itself is pinned, by its decay, not by its effect on a payload.
#[test]
fn acoustic_presets_reverberate_in_proportion_to_their_rt60() {
    let frames = RATE as usize * 2;
    let mut samples = vec![0.0f32; frames * 2];
    samples[0] = 1.0;
    samples[1] = 1.0;
    let impulse = from_interleaved(RATE, 2, &samples).unwrap();

    let tail_ratio = |preset: RoomPreset| {
        let captured = AcousticRerecord::new("probe", preset)
            .apply(&impulse, 11, &UnavailableRunner)
            .unwrap();
        let mono = captured.mono_sum();
        let energy = |from: f64, to: f64| -> f64 {
            let a = (from * f64::from(RATE)) as usize;
            let b = ((to * f64::from(RATE)) as usize).min(mono.len());
            mono[a..b]
                .iter()
                .map(|&s| f64::from(s) * f64::from(s))
                .sum()
        };
        let head = energy(0.0, 0.05);
        assert!(head > 0.0, "the direct path carried no energy at all");
        energy(0.35, 0.45) / head
    };

    let small = tail_ratio(RoomPreset::SMALL);
    let medium = tail_ratio(RoomPreset::MEDIUM);
    let large = tail_ratio(RoomPreset::LARGE);

    assert!(
        small < medium && medium < large,
        "tail energy must grow with RT60: small {small:.3e}, medium {medium:.3e}, large {large:.3e}"
    );
    assert!(
        large > small * 100.0,
        "a 1.4 s room and a 0.28 s room must not be near-identical: {large:.3e} vs {small:.3e}"
    );
}

/// The bench measures whatever it is handed, so a non-finite sample must be refused at the door
/// rather than surface as a recovery rate. `AudioBuffer` is the permissive substrate and accepts a
/// NaN by design; the bench's admission policy is what makes that unmeasurable, and it has to bind
/// on the way in AND on the way out of a channel.
#[test]
fn non_finite_audio_is_refused_at_every_boundary_the_substrate_permits() {
    let mut planes = vec![0.5f32; 4_096 * 2];
    planes[3_000] = f32::NAN;
    let hostile = AudioBuffer::from_planes(RATE, 2, planes).unwrap();
    assert!(
        hostile.planes().iter().any(|s| !s.is_finite()),
        "the substrate type must accept this, or the test proves nothing about the bench's policy"
    );

    let refused = audio_provenance_bench::audio::admit(hostile.clone()).unwrap_err();
    assert_eq!(refused.code(), "audio_non_finite_sample");

    let rejected = CorpusItem::new(
        "hostile",
        ContentClass::HarmonicPad,
        CorpusSource::Synthetic {
            generator: "test".to_owned(),
            recipe: "nan".to_owned(),
        },
        hostile,
    )
    .unwrap_err();
    assert_eq!(
        rejected.code(),
        "audio_non_finite_sample",
        "the corpus constructor is the only way in, so the policy has to bind there"
    );

    // Output side: +800 dB overflows f32 to infinity, so a channel that blows up must error
    // instead of reporting a measurement over garbage.
    let overflowed = Gain::new("gain_absurd", 800.0)
        .apply(&tone(440.0, 0.2), 1, &UnavailableRunner)
        .unwrap_err();
    assert_eq!(overflowed.code(), "audio_non_finite_sample");
}
