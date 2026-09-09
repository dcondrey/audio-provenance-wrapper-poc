//! Load a Watermark-N model card produced by the PyTorch training harness and run the detector
//! over synthesized audio, printing what came back.
//!
//! This is the PyTorch -> Rust loop-closure probe. It asserts nothing about detection quality: an
//! untrained smoke model detects nothing, and `DetectionClass::None` is the correct outcome. What
//! it proves is that the card validated, both graph digests matched, both ONNX sessions built with
//! the declared tensor shapes, and the decoder ran over a dynamic frame count.
//!
//!     cargo run -p apw-watermark-neural --example run_card -- <card.json> [seconds]

use std::error::Error;
use std::path::PathBuf;

use audio_provenance_audio::AudioBuffer;
use apw_watermark_neural::{NeuralWatermark, Payload};

const SAMPLE_RATE: u32 = 48_000;

fn music_like(seconds: f64) -> Vec<f32> {
    let frames = (seconds * f64::from(SAMPLE_RATE)) as usize;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..frames)
        .map(|index| {
            let t = index as f64 / f64::from(SAMPLE_RATE);
            let mut value = 0.0f64;
            for (harmonic, gain) in [(1.0, 0.5), (2.0, 0.25), (3.0, 0.12), (5.0, 0.06)] {
                value += gain * (std::f64::consts::TAU * 174.6 * harmonic * t).sin();
            }
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = ((state >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0;
            ((value + 0.02 * noise) * 0.5) as f32
        })
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let card = PathBuf::from(args.next().ok_or("usage: run_card <card.json> [seconds]")?);
    let seconds: f64 = match args.next() {
        Some(value) => value.parse()?,
        None => 12.0,
    };

    let model = NeuralWatermark::load(&card)?;
    let capabilities = model.capabilities();
    println!("card            {}", card.display());
    println!(
        "model_id        {} epoch {} fixture {}",
        capabilities.model_id, capabilities.epoch, capabilities.is_fixture
    );
    println!(
        "acoustic        {}",
        capabilities.acoustic_rerecording.status()
    );
    println!(
        "band            {:.1} Hz .. {:.1} Hz, {} message bits, can_embed {}",
        capabilities.band_hz.0,
        capabilities.band_hz.1,
        capabilities.message_bits,
        capabilities.can_embed
    );

    let audio = AudioBuffer::from_channels(SAMPLE_RATE, &[music_like(seconds)])?;
    println!(
        "input           {:.2} s at {} Hz, {} channel(s)",
        audio.duration_seconds(),
        audio.sample_rate(),
        audio.channels()
    );

    let detection = model.detect(&audio)?;
    println!(
        "detect(cover)   class {:?} presence_score {:.6} positive {:.3} s frames {} windows {} crc_trials {}",
        detection.class(),
        detection.presence_score(),
        detection.presence_positive_seconds(),
        detection.frames_analysed(),
        detection.windows_examined(),
        detection.crc_trials()
    );

    // The shift-invariance claim, on the Rust side of the boundary. Two crops of the SAME longer
    // signal at offsets 137 samples apart - deliberately not a whole hop, because a whole-hop shift
    // passes even for a detector keying on STFT frame phase. Spec 4.2 says the pooled readout
    // absorbs bulk delay; if these two scores diverge, it does not.
    let long = music_like(seconds + 1.0);
    let window = (seconds * f64::from(SAMPLE_RATE)) as usize;
    let mut scores = Vec::new();
    for offset in [0usize, 137] {
        let crop =
            AudioBuffer::from_channels(SAMPLE_RATE, &[long[offset..offset + window].to_vec()])?;
        scores.push(model.detect(&crop)?.presence_score());
    }
    println!(
        "shift 137 samp  presence {:.6} vs {:.6}, delta {:.3e}",
        scores[0],
        scores[1],
        (scores[0] - scores[1]).abs()
    );

    if capabilities.can_embed {
        let payload = Payload::new(apw_watermark_neural::params::PAYLOAD_VERSION, 3, 0x0155_5555)?;
        let marked = model.embed(&audio, payload)?;
        let residual: f32 = audio
            .channel(0)
            .zip(marked.channel(0))
            .map(|(cover, marked)| {
                cover
                    .iter()
                    .zip(marked.iter())
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0f32, f32::max)
            })
            .unwrap_or(0.0);
        println!(
            "embed           {} frames out, peak residual {:.6} ({:.1} dBFS)",
            marked.frames(),
            residual,
            20.0 * f64::from(residual.max(1e-12)).log10()
        );
        let after = model.detect(&marked)?;
        println!(
            "detect(marked)  class {:?} presence_score {:.6} positive {:.3} s bits_corrected {}",
            after.class(),
            after.presence_score(),
            after.presence_positive_seconds(),
            after.bits_corrected()
        );
    }
    Ok(())
}
