#![allow(clippy::unwrap_used, clippy::panic)]
#![cfg(feature = "codecs")]

use audio_provenance_audio::decode::{DecodeLimits, decode_bytes};
use audio_provenance_core::CodedError;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}{name}")).unwrap()
}

fn reference() -> Vec<f32> {
    fixture("sine_s16.reference.f32")
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes(word.try_into().unwrap()))
        .collect()
}

/// Both fixtures are lossless transcodes of the same source, so the symphonia
/// path has to land on the very same samples the strict WAV reader does. This
/// covers the parts of that adapter that are easy to get subtly wrong: picking
/// the audio track, reading the spec, and re-interleaving the planes.
#[test]
fn lossless_non_wav_containers_decode_to_the_same_samples() {
    let expected = reference();
    for name in ["sine.flac", "sine_s16.aiff"] {
        let decoded = decode_bytes(&fixture(name), &DecodeLimits::default()).unwrap();
        assert_eq!(decoded.sample_rate(), 44_100, "{name}");
        assert_eq!(decoded.channels(), 2, "{name}");
        assert_eq!(decoded.frames(), 11_025, "{name}");
        let interleaved = decoded.to_interleaved();
        assert_eq!(interleaved.len(), expected.len(), "{name}");
        let error = interleaved
            .iter()
            .zip(expected.iter())
            .fold(0.0f32, |acc, (a, b)| acc.max((a - b).abs()));
        assert!(
            error < 1.0e-6,
            "{name} differs from the reference by {error}"
        );
    }
}

/// A hostile AIFF is rejected on its own chunk table before symphonia ever sees
/// it, so a declared length that outruns the file cannot size a decoder buffer.
#[test]
fn hostile_aiff_headers_are_rejected_before_decoding() {
    let base = fixture("sine_s16.aiff");

    let mut bytes = base.clone();
    bytes[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
    let error = decode_bytes(&bytes, &DecodeLimits::default()).unwrap_err();
    assert_eq!(error.code(), "audio_chunk_truncated");

    let mut bytes = base.clone();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == b"SSND" {
            bytes[at + 4..at + 8].copy_from_slice(&u32::MAX.to_be_bytes());
            break;
        }
        at += 8 + len + (len & 1);
    }
    let error = decode_bytes(&bytes, &DecodeLimits::default()).unwrap_err();
    assert_eq!(error.code(), "audio_chunk_truncated");

    let mut bytes = base.clone();
    bytes[8..12].copy_from_slice(b"XXXX");
    let error = decode_bytes(&bytes, &DecodeLimits::default()).unwrap_err();
    assert_eq!(error.code(), "audio_container_unsupported");
}

#[test]
fn unrecognised_input_is_a_typed_error() {
    let error =
        decode_bytes(b"not audio at all, just bytes", &DecodeLimits::default()).unwrap_err();
    assert!(
        matches!(
            error.code(),
            "audio_container_unsupported" | "audio_decode_failed"
        ),
        "unexpected code {}",
        error.code()
    );
}
