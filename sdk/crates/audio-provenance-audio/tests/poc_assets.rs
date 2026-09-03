#![allow(clippy::unwrap_used, clippy::panic)]
#![cfg(feature = "fs")]

use std::path::Path;

use audio_provenance_audio::decode::DecodeLimits;
use audio_provenance_audio::files::decode_file;
use audio_provenance_audio::metrics::{crest_factor, energy_envelope, rms, zero_crossing_rate};

const POC: &str = "/Volumes/A/audio-provenance/demo-output";

/// Shape figures come from `ffmpeg -i` on each file; the format column is the
/// reason each one is here, 32-bit float most of all.
const REAL_FILES: [(&str, &str, u32, usize, usize); 3] = [
    (
        "samples/demo-source.wav",
        "pcm_s24le",
        44_100,
        2,
        44_100 * 8,
    ),
    (
        "sessions/capture-20260828T220130Z-87760/exports/demo-source-bounce.wav",
        "pcm_s16le",
        44_100,
        2,
        44_100 * 8,
    ),
    (
        "sessions/capture-20260830T021208Z-11629/exports/session-b-bounce.wav",
        "pcm_f32le",
        44_100,
        2,
        44_100 * 24,
    ),
];

#[test]
fn real_poc_exports_decode_to_their_declared_shape() {
    for (relative, format, rate, channels, frames) in REAL_FILES {
        let path = format!("{POC}/{relative}");
        let buffer = decode_file(Path::new(&path), &DecodeLimits::default()).unwrap();
        assert_eq!(buffer.sample_rate(), rate, "{relative} ({format})");
        assert_eq!(buffer.channels(), channels, "{relative} ({format})");
        assert_eq!(buffer.frames(), frames, "{relative} ({format})");
        assert!(
            (buffer.duration_seconds() - frames as f64 / f64::from(rate)).abs() < 1.0e-9,
            "{relative}"
        );
    }
}

/// The POC's association reader is Python's `wave` module, which handles integer
/// PCM only, so its signer refuses `WAVE_FORMAT_IEEE_FLOAT` outright and every
/// float export graded `unavailable`. This is the file that did it; it has to
/// decode, and the samples have to arrive untouched.
#[test]
fn float32_export_decodes_and_is_a_bit_exact_passthrough() {
    let path =
        format!("{POC}/sessions/capture-20260830T021208Z-11629/exports/session-b-bounce.wav");
    let buffer = decode_file(Path::new(&path), &DecodeLimits::default()).unwrap();
    let bytes = std::fs::read(&path).unwrap();

    let mut at = 12;
    let mut data = None;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"data" {
            data = Some(&bytes[at + 8..at + 8 + len]);
            break;
        }
        at += 8 + len + (len & 1);
    }
    let data = data.unwrap();

    let interleaved = buffer.to_interleaved();
    assert_eq!(interleaved.len() * 4, data.len());
    for (index, (sample, word)) in interleaved.iter().zip(data.chunks_exact(4)).enumerate() {
        let raw = f32::from_le_bytes(word.try_into().unwrap());
        assert_eq!(
            sample.to_bits(),
            raw.to_bits(),
            "sample {index} was not passed through unchanged"
        );
    }
}

/// The expected values were produced by `daemon/audio_association.py::_feature`
/// through the POC's own venv, over the same interleaved slice. That code has no
/// knowledge of this crate, so agreement is cross-language evidence rather than
/// a restatement of the implementation.
#[test]
fn metrics_match_the_poc_feature_extractor() {
    const OFFSET: usize = 88_200;
    const LEN: usize = 4_096;
    const EXPECTED_RMS: f64 = 0.027_582_973_528_887_29;
    const EXPECTED_ZCR: f64 = 0.007_326_007_326_007_326;
    const EXPECTED_CREST: f64 = 2.328_954_921_623_943_4;
    const EXPECTED_ENVELOPE: [f64; 4] = [
        0.836_100_410_473_176_1,
        0.967_008_704_576_474_3,
        1.066_633_353_599_693_2,
        1.108_207_362_305_713_8,
    ];

    let path =
        format!("{POC}/sessions/capture-20260828T220130Z-87760/exports/demo-source-bounce.wav");
    let buffer = decode_file(Path::new(&path), &DecodeLimits::default()).unwrap();
    let interleaved = buffer.to_interleaved();
    let window = &interleaved[OFFSET..OFFSET + LEN];

    assert!(
        (rms(window) - EXPECTED_RMS).abs() < 1.0e-12,
        "{}",
        rms(window)
    );
    assert!(
        (zero_crossing_rate(window) - EXPECTED_ZCR).abs() < 1.0e-15,
        "{}",
        zero_crossing_rate(window)
    );
    assert!(
        (crest_factor(window) - EXPECTED_CREST).abs() < 1.0e-10,
        "{}",
        crest_factor(window)
    );
    for (found, expected) in energy_envelope(window).iter().zip(EXPECTED_ENVELOPE) {
        assert!((found - expected).abs() < 1.0e-10, "{found} vs {expected}");
    }
}

/// `AudioObserver::pushAudioBlock` sums channels in f32 and multiplies by a
/// precomputed `1 / channels`. Summing in f64 or dividing per sample changes the
/// low bits, and the hash chain is taken over exactly these bytes.
#[test]
fn mono_sum_matches_the_observer_accumulation() {
    let path =
        format!("{POC}/sessions/capture-20260830T021208Z-11629/exports/session-b-bounce.wav");
    let buffer = decode_file(Path::new(&path), &DecodeLimits::default()).unwrap();
    let mono = buffer.mono_sum();
    assert_eq!(mono.len(), buffer.frames());

    let left = buffer.channel(0).unwrap();
    let right = buffer.channel(1).unwrap();
    for frame in 0..buffer.frames() {
        let narrow = (left[frame] + right[frame]) * 0.5f32;
        assert_eq!(mono[frame].to_bits(), narrow.to_bits(), "frame {frame}");
    }
}

/// Two channels cannot detect a widened accumulator: an f32 sum of two f32s and
/// an f64 sum rounded to f32 always agree, and halving is exact. Three channels
/// can, and `1 / 3` is not exact either, so this is where both halves of the POC
/// accumulation actually bind.
#[test]
fn mono_sum_accumulator_width_and_reciprocal_are_load_bearing() {
    const FRAMES: usize = 4_096;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut planes = vec![0.0f32; FRAMES * 3];
    for slot in planes.iter_mut() {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *slot = ((state >> 33) as f32 / (1u64 << 31) as f32) - 1.0;
    }
    let buffer = audio_provenance_audio::AudioBuffer::from_planes(48_000, 3, planes).unwrap();
    let mono = buffer.mono_sum();

    let gain = 1.0f32 / 3.0f32;
    let mut widened_differs = 0usize;
    let mut divided_differs = 0usize;
    for (frame, found) in mono.iter().enumerate() {
        let mut sum = 0.0f32;
        for channel in 0..3 {
            sum += buffer.channel(channel).unwrap()[frame];
        }
        assert_eq!(found.to_bits(), (sum * gain).to_bits(), "frame {frame}");

        let widened = (0..3)
            .map(|channel| f64::from(buffer.channel(channel).unwrap()[frame]))
            .sum::<f64>();
        if ((widened / 3.0) as f32).to_bits() != found.to_bits() {
            widened_differs += 1;
        }
        if (sum / 3.0f32).to_bits() != found.to_bits() {
            divided_differs += 1;
        }
    }
    assert!(
        widened_differs > 0,
        "an f64 accumulator was indistinguishable across {FRAMES} frames"
    );
    assert!(
        divided_differs > 0,
        "dividing instead of multiplying by the reciprocal was indistinguishable"
    );
}
