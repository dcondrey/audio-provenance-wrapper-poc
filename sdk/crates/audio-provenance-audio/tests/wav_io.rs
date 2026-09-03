#![allow(clippy::unwrap_used, clippy::panic)]

use audio_provenance_audio::buffer::AudioBuffer;
use audio_provenance_audio::decode::DecodeLimits;
use audio_provenance_audio::wav::{BitDepth, decode, encode};
use audio_provenance_core::CodedError;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{FIXTURES}{name}")).unwrap()
}

/// Byte offset of a top-level chunk's payload, found by walking the real file
/// rather than by assuming a writer's layout.
fn chunk_payload_offset(bytes: &[u8], id: &[u8; 4]) -> usize {
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if &bytes[at..at + 4] == id {
            return at + 8;
        }
        at += 8 + len + (len & 1);
    }
    panic!("chunk not found in fixture");
}

fn patch_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + 2].copy_from_slice(&value.to_le_bytes());
}

fn patch_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn ramp(frames: usize, channels: usize) -> AudioBuffer {
    let mut planes = vec![0.0f32; frames * channels];
    for channel in 0..channels {
        for frame in 0..frames {
            let position = frame as f32 / frames as f32;
            planes[channel * frames + frame] =
                (position * 2.0 - 1.0) * if channel == 0 { 0.9 } else { -0.6 };
        }
    }
    AudioBuffer::from_planes(48_000, channels, planes).unwrap()
}

#[test]
fn wav_round_trips_at_every_supported_depth() {
    let source = ramp(2_048, 2);
    for (depth, tolerance) in [
        (BitDepth::Int16, 1.0 / 32_768.0),
        (BitDepth::Int24, 1.0 / 8_388_608.0),
        // 32-bit int exceeds the f32 mantissa, so the round trip is limited by
        // the sample type rather than by the container.
        (BitDepth::Int32, 1.0e-7),
        (BitDepth::Float32, 0.0),
    ] {
        let bytes = encode(&source, depth).unwrap();
        let restored = decode(&bytes, &DecodeLimits::default()).unwrap();
        assert_eq!(restored.sample_rate(), source.sample_rate(), "{depth:?}");
        assert_eq!(restored.channels(), source.channels(), "{depth:?}");
        assert_eq!(restored.frames(), source.frames(), "{depth:?}");
        for channel in 0..source.channels() {
            let expected = source.channel(channel).unwrap();
            let found = restored.channel(channel).unwrap();
            let error = expected
                .iter()
                .zip(found.iter())
                .fold(0.0f32, |acc, (a, b)| acc.max((a - b).abs()));
            assert!(error <= tolerance, "{depth:?} round trip error {error}");
        }
    }
}

/// ffmpeg wrote both the fixture and the reference, so agreement here is
/// agreement with an outside decoder, not with this crate's own encoder.
#[test]
fn decoding_agrees_with_ffmpeg() {
    let decoded = decode(&fixture("sine_s16.wav"), &DecodeLimits::default()).unwrap();
    let raw = fixture("sine_s16.reference.f32");
    let reference: Vec<f32> = raw
        .chunks_exact(4)
        .map(|word| f32::from_le_bytes(word.try_into().unwrap()))
        .collect();

    assert_eq!(decoded.sample_rate(), 44_100);
    assert_eq!(decoded.channels(), 2);
    assert_eq!(decoded.frames() * decoded.channels(), reference.len());
    let interleaved = decoded.to_interleaved();
    let error = interleaved
        .iter()
        .zip(reference.iter())
        .fold(0.0f32, |acc, (a, b)| acc.max((a - b).abs()));
    assert!(error < 1.0e-6, "decode differs from ffmpeg by {error}");
}

#[test]
fn every_supported_wav_format_decodes() {
    for name in [
        "sine_s16.wav",
        "sine_s24.wav",
        "sine_s32.wav",
        "sine_f32.wav",
    ] {
        let decoded = decode(&fixture(name), &DecodeLimits::default()).unwrap();
        assert_eq!(decoded.sample_rate(), 44_100, "{name}");
        assert_eq!(decoded.channels(), 2, "{name}");
        assert_eq!(decoded.frames(), 11_025, "{name}");
        let left = decoded.channel(0).unwrap();
        let right = decoded.channel(1).unwrap();
        let left_peak = left.iter().fold(0.0f32, |acc, v| acc.max(v.abs()));
        let right_peak = right.iter().fold(0.0f32, |acc, v| acc.max(v.abs()));
        assert!(left_peak > 0.85, "{name} left peak {left_peak}");
        // The fixture's right channel is the left at half amplitude, so a
        // planar/interleaved mix-up shows up as equal peaks.
        assert!(
            (right_peak / left_peak - 0.5).abs() < 0.02,
            "{name} channel ratio {}",
            right_peak / left_peak
        );
    }
}

/// Every case here is a byte-patched copy of an ffmpeg-written file. Each must
/// come back as a typed error rather than a panic, a hang, or a huge allocation.
#[test]
fn hostile_wav_headers_yield_typed_errors() {
    let base = fixture("sine_s16.wav");
    let fmt = chunk_payload_offset(&base, b"fmt ");
    let data = chunk_payload_offset(&base, b"data");
    let limits = DecodeLimits::default();

    let mut cases: Vec<(&str, Vec<u8>, &str)> = Vec::new();

    let mut bytes = base.clone();
    patch_u32(&mut bytes, data - 4, (base.len() * 4) as u32);
    cases.push((
        "data size beyond end of file",
        bytes,
        "audio_chunk_truncated",
    ));

    let mut bytes = base.clone();
    patch_u32(&mut bytes, data - 4, u32::MAX);
    cases.push(("data size 0xFFFFFFFF", bytes, "audio_chunk_truncated"));

    let mut bytes = base.clone();
    patch_u32(&mut bytes, 4, u32::MAX);
    cases.push(("riff size 0xFFFFFFFF", bytes, "audio_chunk_truncated"));

    let mut bytes = base.clone();
    let declared = u32::from_le_bytes(bytes[data - 4..data].try_into().unwrap());
    patch_u32(&mut bytes, data - 4, declared - 1);
    cases.push((
        "data chunk truncated mid frame",
        bytes,
        "audio_chunk_truncated",
    ));

    let mut bytes = base.clone();
    patch_u32(&mut bytes, fmt - 4, 8);
    cases.push((
        "fmt chunk shorter than 16 bytes",
        bytes,
        "audio_header_malformed",
    ));

    let mut bytes = base.clone();
    patch_u16(&mut bytes, fmt + 2, 0);
    cases.push(("zero channels", bytes, "audio_channel_count_invalid"));

    let mut bytes = base.clone();
    patch_u16(&mut bytes, fmt + 2, 4096);
    cases.push(("absurd channel count", bytes, "audio_channel_count_invalid"));

    let mut bytes = base.clone();
    patch_u32(&mut bytes, fmt + 4, 0);
    cases.push(("zero sample rate", bytes, "audio_sample_rate_invalid"));

    let mut bytes = base.clone();
    patch_u16(&mut bytes, fmt + 14, 0);
    cases.push((
        "zero bits per sample",
        bytes,
        "audio_sample_format_unsupported",
    ));

    let mut bytes = base.clone();
    patch_u16(&mut bytes, fmt + 14, 255);
    cases.push((
        "255 bits per sample",
        bytes,
        "audio_sample_format_unsupported",
    ));

    let mut bytes = base.clone();
    patch_u16(&mut bytes, fmt + 12, 3);
    cases.push((
        "block align disagrees with the format",
        bytes,
        "audio_header_malformed",
    ));

    // Renaming a chunk keeps its length field valid, so the walker strides past
    // it correctly and the decoder reaches the genuinely-missing-chunk path.
    let mut bytes = base.clone();
    bytes[data - 8..data - 4].copy_from_slice(b"junk");
    cases.push(("no data chunk", bytes, "audio_chunk_missing"));

    let mut bytes = base.clone();
    bytes[fmt - 8..fmt - 4].copy_from_slice(b"junk");
    cases.push(("no fmt chunk", bytes, "audio_chunk_missing"));

    cases.push((
        "file truncated to the form header",
        base[..8].to_vec(),
        "audio_header_malformed",
    ));

    cases.push(("empty input", Vec::new(), "audio_header_malformed"));

    for (name, bytes, expected) in cases {
        match decode(&bytes, &limits) {
            Ok(_) => panic!("{name} was accepted"),
            Err(error) => assert_eq!(error.code(), expected, "{name}: {error}"),
        }
    }
}

#[test]
fn decode_limits_bound_the_allocation() {
    let bytes = fixture("sine_s32.wav");
    let limits = DecodeLimits::new(DecodeLimits::DEFAULT_MAX_BYTES, 1_024);
    let error = decode(&bytes, &limits).unwrap_err();
    assert_eq!(error.code(), "audio_sample_limit_exceeded");

    let limits = DecodeLimits::new(64, DecodeLimits::DEFAULT_MAX_SAMPLES);
    let error = decode(&bytes, &limits).unwrap_err();
    assert_eq!(error.code(), "audio_byte_limit_exceeded");
}
