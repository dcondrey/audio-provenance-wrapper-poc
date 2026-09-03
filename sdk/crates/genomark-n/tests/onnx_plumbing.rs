//! Proves the inference path executes a real ONNX graph, with no trained model available.
//!
//! The fixtures are hand-written arithmetic graphs emitted by `tools/make_fixture_models.py`. Every
//! value they produce is computable by hand from the input, so these assertions pin numbers rather
//! than "it ran". They say nothing whatever about watermarking: a future engineer swaps in trained
//! weights and a real card, and the path below is already exercised.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use audio_provenance_audio::AudioBuffer;
use audio_provenance_core::CodedError;
use genomark_n::model::GenoMarkN;
use genomark_n::params::{BAND_BINS, MESSAGE_BITS, PAYLOAD_VERSION, UNSUPPORTED};
use genomark_n::payload::Payload;
use genomark_n::session::DecoderSession;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn card_path() -> PathBuf {
    fixtures().join("genomark-n-fixture-v1.card.json")
}

/// `tools/make_fixture_models.py` writes this vector into the decoder graph as an initializer.
fn coefficient(index: usize) -> f64 {
    (index as f64 - 27.5) / 8.0
}

fn ramp_tensor(time: usize) -> Vec<f32> {
    let mut tensor = vec![0.0f32; BAND_BINS * time];
    for row in 0..BAND_BINS {
        for t in 0..time {
            tensor[row * time + t] = ((row + t) % 7) as f32 * 0.25 - 0.5;
        }
    }
    tensor
}

/// The decoder graph runs, the dynamic time axis really is dynamic, and the outputs are the exact
/// arithmetic the graph declares. This is the assertion that makes the crate a deliverable rather
/// than something that merely compiles.
#[test]
fn runs_the_onnx_decoder_and_reproduces_its_arithmetic_at_two_lengths() {
    let card = genomark_n::ModelCard::read(&card_path()).unwrap();
    let bytes = card
        .read_graph(&fixtures(), "decoder", &card.graphs.decoder)
        .unwrap();
    let decoder = DecoderSession::new(
        &bytes,
        &card.io.decoder_input,
        &card.io.decoder_presence_output,
        &card.io.decoder_message_output,
    )
    .unwrap();

    for time in [13usize, 41] {
        let tensor = ramp_tensor(time);
        let output = decoder.run(&tensor, time).unwrap();
        assert_eq!(output.presence_logits.len(), time);

        let mut total = 0.0f64;
        for t in 0..time {
            let expected: f64 = (0..BAND_BINS)
                .map(|row| f64::from(tensor[row * time + t]))
                .sum::<f64>()
                / BAND_BINS as f64;
            assert!(
                (f64::from(output.presence_logits[t]) - expected).abs() < 1e-5,
                "presence frame {t} at T={time}: {} vs {expected}",
                output.presence_logits[t]
            );
            total += expected;
        }
        let global = total / time as f64;
        for index in 0..MESSAGE_BITS {
            let expected = global * coefficient(index);
            assert!(
                (f64::from(output.message_logits[index]) - expected).abs() < 1e-5,
                "message bit {index} at T={time}: {} vs {expected}",
                output.message_logits[index]
            );
        }
    }
}

/// IMPORTANT: the model file is trusted code-equivalent. A swapped model mints accepts, so a
/// digest mismatch must be refused rather than warned about.
#[test]
fn refuses_a_graph_whose_digest_does_not_match_its_card() {
    let directory = tempdir();
    let card_text = std::fs::read_to_string(card_path()).unwrap();
    std::fs::write(
        directory.join("genomark-n-fixture-v1.card.json"),
        &card_text,
    )
    .unwrap();
    for name in ["fixture-decoder-v1.onnx", "fixture-encoder-v1.onnx"] {
        std::fs::write(
            directory.join(name),
            std::fs::read(fixtures().join(name)).unwrap(),
        )
        .unwrap();
    }
    let target = directory.join("fixture-decoder-v1.onnx");
    let mut bytes = std::fs::read(&target).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&target, &bytes).unwrap();

    let error = GenoMarkN::load(&directory.join("genomark-n-fixture-v1.card.json"))
        .expect_err("a tampered graph must not load");
    assert_eq!(error.code(), "model_weights_hash_mismatch");
}

/// An untrained build must be STRUCTURALLY incapable of reporting the acoustic path as supported.
#[test]
fn a_fixture_card_reports_the_acoustic_path_unsupported() {
    let model = GenoMarkN::load(&card_path()).unwrap();
    let capabilities = model.capabilities();
    assert!(capabilities.is_fixture);
    assert_eq!(capabilities.acoustic_rerecording.status(), UNSUPPORTED);
    assert!(!capabilities.acoustic_rerecording.is_supported());
    assert!(capabilities.acoustic_rerecording.envelope().is_none());
    assert_eq!(capabilities.payload_bits, 32);
    assert_eq!(capabilities.band_hz.0.round() as i64, 211);
    assert_eq!(capabilities.band_hz.1.round() as i64, 7688);
}

/// The whole detect path: mono sum, STFT, one dense decoder pass over the entire signal, presence
/// pooling, then the sliding locator windows and the flip search. The fixture's message logits have
/// fixed signs, so the decode is deterministic and rejects. The resample arm of the path is covered
/// by the embed test, which is handed 44.1 kHz.
#[test]
fn detects_end_to_end_over_a_forty_second_signal_and_accepts_nothing() {
    let model = GenoMarkN::load(&card_path()).unwrap();
    let audio = noise(48_000, 2, 48_000 * 40, 0.5);
    let outcome = model.detect(&audio).unwrap();

    assert_eq!(outcome.class(), genomark_n::DetectionClass::None);
    assert_eq!(outcome.payload(), None);
    assert_eq!(outcome.confidence(), 0.0);
    assert_eq!(outcome.windows_examined(), 2);
    assert_eq!(outcome.crc_trials(), 22);
    assert!(
        outcome.presence_score() > 0.5 && outcome.presence_score() < 0.995,
        "presence score {}",
        outcome.presence_score()
    );
    assert!(outcome.presence_positive_seconds() > 30.0);
}

/// The embed path runs the encoder graph, applies the budget-limited mask, forms the residual at
/// 48 kHz and resamples it back. The host's shape survives and the residual is a perturbation.
#[test]
fn embeds_through_the_onnx_encoder_without_changing_the_host_shape() {
    let model = GenoMarkN::load(&card_path()).unwrap();
    assert!(model.capabilities().can_embed);
    let host = noise(44_100, 2, 44_100 * 5, 0.25);
    let payload = Payload::new(PAYLOAD_VERSION, 6, 0x0012_3456).unwrap();
    let marked = model.embed(&host, payload).unwrap();

    assert_eq!(marked.sample_rate(), host.sample_rate());
    assert_eq!(marked.channels(), host.channels());
    assert_eq!(marked.frames(), host.frames());

    let mut peak_residual = 0.0f32;
    let mut peak_host = 0.0f32;
    for channel in 0..host.channels() {
        let cover = host.channel(channel).unwrap();
        let test = marked.channel(channel).unwrap();
        for (a, b) in cover.iter().zip(test.iter()) {
            peak_residual = peak_residual.max((b - a).abs());
            peak_host = peak_host.max(a.abs());
        }
    }
    assert!(peak_residual > 0.0, "the encoder wrote nothing");
    assert!(
        peak_residual < peak_host,
        "residual {peak_residual} is not a perturbation of a {peak_host} host"
    );
}

fn noise(sample_rate: u32, channels: usize, frames: usize, amplitude: f32) -> AudioBuffer {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut planes = Vec::with_capacity(channels * frames);
    for _ in 0..channels * frames {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let unit = (state >> 11) as f32 / (1u64 << 53) as f32;
        planes.push((unit * 2.0 - 1.0) * amplitude);
    }
    AudioBuffer::from_planes(sample_rate, channels, planes).unwrap()
}

fn tempdir() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "genomark-n-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

/// The bench measures GenoMark-N through the same trait object it measures GenoMark-Q through, so
/// the existing matrix runs it with no bench changes. `payload_len` is the INFORMATION bytes only:
/// handing the CRC back through the payload would dilute every bit-error-rate row by 24 bits the
/// caller never chose.
#[cfg(feature = "bench")]
#[test]
fn presents_the_same_interface_shape_as_the_classical_mark() {
    use audio_provenance_bench::WatermarkCodec;

    let codec = genomark_n::GenoMarkNCodec::load(&card_path()).unwrap();
    let handle: &dyn WatermarkCodec = &codec;
    assert_eq!(handle.name(), genomark_n::ALGORITHM_ID);
    assert_eq!(handle.payload_len(), genomark_n::PAYLOAD_BYTES);
    assert!(handle.is_bench_fixture());
    assert!(handle.describe().contains(UNSUPPORTED));

    let host = noise(48_000, 1, 48_000 * 3, 0.25);

    // `BenchConfig::default()` carries a payload whose leading bits are not this build's version,
    // and the decoder's syntactic check would refuse it. An operator must see that at embed rather
    // than as a floor of zeros in every recovery row.
    assert!(handle.embed(&host, &[0x47, 0x54, 0x01, 0x9A]).is_err());

    let payload = Payload::new(PAYLOAD_VERSION, 1, 7).unwrap().to_bytes();
    let marked = handle.embed(&host, &payload).unwrap();
    let detection = handle.detect(&marked).unwrap();
    // Three seconds is under every window this card declares, so there is nothing to report and
    // the detector declines rather than guessing.
    assert!(!detection.is_accept());
    assert_eq!(detection.confidence, 0.0);
}
