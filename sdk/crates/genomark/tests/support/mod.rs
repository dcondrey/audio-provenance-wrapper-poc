#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use audio_provenance_audio::AudioBuffer;

/// Deterministic broadband test material: a small harmonic stack, a slow sweep and shaped noise, so
/// every cell in the band carries energy at every frame. No dependency on ffmpeg or on a fixture
/// file, and identical on every platform.
pub fn tone_bed(sample_rate: u32, seconds: f64, channels: usize) -> AudioBuffer {
    let frames = (seconds * f64::from(sample_rate)).round() as usize;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut planes = Vec::with_capacity(channels);
    for channel in 0..channels {
        let detune = 1.0 + 0.004 * channel as f64;
        let mut plane = Vec::with_capacity(frames);
        let mut pink = 0.0f64;
        for frame in 0..frames {
            let t = frame as f64 / f64::from(sample_rate);
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let white = ((state >> 33) as f64 / f64::from(u32::MAX >> 1)) - 1.0;
            pink = 0.97 * pink + 0.03 * white;
            let harmonics: f64 = (1..=11)
                .map(|k| {
                    let f = 220.0 * detune * k as f64;
                    (1.0 / k as f64) * (core::f64::consts::TAU * f * t).sin()
                })
                .sum();
            let sweep = (core::f64::consts::TAU * (900.0 + 2600.0 * (0.13 * t).sin()) * t).sin();
            let value = 0.16 * harmonics + 0.10 * sweep + 0.30 * (white * 0.35 + pink * 3.0);
            plane.push((value * 0.5).clamp(-1.0, 1.0) as f32);
        }
        planes.push(plane);
    }
    AudioBuffer::from_channels(sample_rate, &planes).expect("test bed is a valid buffer")
}

pub fn scaled(audio: &AudioBuffer, gain: f32) -> AudioBuffer {
    let planes: Vec<Vec<f32>> = (0..audio.channels())
        .map(|channel| {
            audio
                .channel(channel)
                .expect("channel in range")
                .iter()
                .map(|sample| sample * gain)
                .collect()
        })
        .collect();
    AudioBuffer::from_channels(audio.sample_rate(), &planes).expect("scaling preserves shape")
}

pub fn pure_tone(sample_rate: u32, seconds: f64, hz: f64) -> AudioBuffer {
    let frames = (seconds * f64::from(sample_rate)).round() as usize;
    let plane: Vec<f32> = (0..frames)
        .map(|frame| {
            let t = frame as f64 / f64::from(sample_rate);
            (0.5 * (core::f64::consts::TAU * hz * t).sin()) as f32
        })
        .collect();
    AudioBuffer::from_channels(sample_rate, &[plane.clone(), plane]).expect("valid buffer")
}

pub fn white_noise(sample_rate: u32, seconds: f64) -> AudioBuffer {
    let frames = (seconds * f64::from(sample_rate)).round() as usize;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let plane: Vec<f32> = (0..frames)
        .map(|_| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((((state >> 33) as f64 / f64::from(u32::MAX >> 1)) - 1.0) * 0.35) as f32
        })
        .collect();
    AudioBuffer::from_channels(sample_rate, &[plane.clone(), plane]).expect("valid buffer")
}

/// Deterministic additive white noise at a signal-to-noise ratio measured over the whole buffer.
#[allow(dead_code)]
pub fn noised(audio: &AudioBuffer, snr_db: f64) -> AudioBuffer {
    let mut power = 0.0f64;
    let mut count = 0usize;
    for channel in 0..audio.channels() {
        for sample in audio.channel(channel).expect("channel in range") {
            power += f64::from(*sample) * f64::from(*sample);
            count += 1;
        }
    }
    let rms = if count > 0 {
        (power / count as f64).sqrt()
    } else {
        0.0
    };
    let sigma = rms * 10f64.powf(-snr_db / 20.0);
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let planes: Vec<Vec<f32>> = (0..audio.channels())
        .map(|channel| {
            audio
                .channel(channel)
                .expect("channel in range")
                .iter()
                .map(|sample| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1_442_695_040_888_963_407);
                    let uniform = ((state >> 33) as f64 / f64::from(u32::MAX >> 1)) - 1.0;
                    (f64::from(*sample) + sigma * uniform * 1.732).clamp(-1.0, 1.0) as f32
                })
                .collect()
        })
        .collect();
    AudioBuffer::from_channels(audio.sample_rate(), &planes).expect("noise preserves shape")
}
