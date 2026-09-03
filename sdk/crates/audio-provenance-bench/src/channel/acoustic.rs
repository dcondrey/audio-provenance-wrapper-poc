use super::{Channel, ChannelFamily, Params, param};
use crate::audio::{from_channels, map_samples, mean_square};
use crate::dsp::Rng;
use crate::error::{AudioError, ChannelError};
use crate::ports::CommandRunner;
use audio_provenance_audio::AudioBuffer;
use audio_provenance_audio::biquad::{BiquadCoefficients, cascade};
use audio_provenance_audio::convolve::convolve;
use audio_provenance_audio::resample::resample_ratio;

pub const SIMULATION_DISCLAIMER: &str = "SIMULATED acoustic path. The impulse response is \
synthesised (exponentially decaying noise plus discrete early reflections), the transducer curve is \
a fixed filter cascade, and the room noise is Gaussian. This row is NOT a speaker-to-microphone \
measurement and may not be reported as one. A real over-the-air result requires playing the file \
through a loudspeaker and capturing it with a microphone in a room.";

const MAX_IR_SECONDS: f64 = 3.0;
const SPEED_OF_SOUND: f64 = 343.0;

#[derive(Debug, Clone, Copy)]
pub struct RoomPreset {
    pub rt60_seconds: f64,
    pub source_distance_m: f64,
    pub early_reflections: usize,
    pub room_noise_snr_db: f64,
    pub clock_drift: f64,
    pub direct_to_reverberant_db: f64,
}

impl RoomPreset {
    pub const SMALL: Self = Self {
        rt60_seconds: 0.28,
        source_distance_m: 0.5,
        early_reflections: 12,
        room_noise_snr_db: 40.0,
        clock_drift: 1.000_1,
        direct_to_reverberant_db: 8.0,
    };

    pub const MEDIUM: Self = Self {
        rt60_seconds: 0.60,
        source_distance_m: 2.0,
        early_reflections: 18,
        room_noise_snr_db: 32.0,
        clock_drift: 1.000_2,
        direct_to_reverberant_db: 2.0,
    };

    pub const LARGE: Self = Self {
        rt60_seconds: 1.40,
        source_distance_m: 6.0,
        early_reflections: 24,
        room_noise_snr_db: 26.0,
        clock_drift: 0.999_7,
        direct_to_reverberant_db: -4.0,
    };
}

/// Synthesised room impulse response: a direct tap, discrete early reflections whose delays follow
/// the room's dimensions, and a late field of exponentially decaying noise split into two bands so
/// high frequencies decay faster than low, as they do in a real room.
pub fn synthesise_rir(
    preset: RoomPreset,
    sample_rate: u32,
    seed: u64,
) -> Result<Vec<f32>, AudioError> {
    let rate = f64::from(sample_rate);
    let rt60 = preset.rt60_seconds.clamp(0.05, MAX_IR_SECONDS);
    let length = (rt60 * rate).round().max(16.0) as usize;
    let mut ir = vec![0.0f32; length];
    let mut rng = Rng::new(seed ^ 0x5249_525f_5345_4544);

    let pre_delay = (preset.source_distance_m / SPEED_OF_SOUND * rate).round() as usize;
    let direct_index = pre_delay.min(length - 1);
    ir[direct_index] = 1.0;

    let reverberant = 10f32.powf((-preset.direct_to_reverberant_db / 20.0) as f32);
    let early_window = (0.08 * rate) as usize;
    for k in 0..preset.early_reflections {
        let jitter = f64::from(rng.next_unit());
        let extra = (1.0 + k as f64) * 0.004 + jitter * 0.012;
        let index = direct_index + (extra * rate).round() as usize;
        if index >= length || index > direct_index + early_window {
            continue;
        }
        let travel = 1.0 + extra * SPEED_OF_SOUND / preset.source_distance_m.max(0.1);
        let amplitude = reverberant * (1.0 / travel as f32) * rng.next_symmetric().signum();
        ir[index] += amplitude * (0.6 + 0.4 * rng.next_unit());
    }

    let mut low = vec![0.0f32; length];
    let mut high = vec![0.0f32; length];
    let hf_rt60 = rt60 * 0.55;
    for n in 0..length {
        let t = n as f64 / rate;
        let low_env = (-6.907_755 * t / rt60).exp() as f32;
        let high_env = (-6.907_755 * t / hf_rt60).exp() as f32;
        low[n] = rng.next_gaussian() * low_env;
        high[n] = rng.next_gaussian() * high_env;
    }
    cascade(
        &[BiquadCoefficients::low_pass(sample_rate, 1_500.0, 0.707)?],
        &mut low,
    );
    cascade(
        &[BiquadCoefficients::high_pass(sample_rate, 1_500.0, 0.707)?],
        &mut high,
    );

    let tail_energy: f32 = low
        .iter()
        .zip(high.iter())
        .map(|(a, b)| (a + b) * (a + b))
        .sum::<f32>()
        .max(f32::MIN_POSITIVE);
    let tail_scale = reverberant / tail_energy.sqrt();
    for n in 0..length {
        if n >= direct_index {
            ir[n] += (low[n - direct_index] + high[n - direct_index]) * tail_scale;
        }
    }
    Ok(ir)
}

#[derive(Debug, Clone)]
pub struct AcousticRerecord {
    name: String,
    preset: RoomPreset,
}

impl AcousticRerecord {
    pub fn new(name: impl Into<String>, preset: RoomPreset) -> Self {
        Self {
            name: name.into(),
            preset,
        }
    }
}

fn transducer_chain(sample_rate: u32) -> Result<[BiquadCoefficients; 4], AudioError> {
    Ok([
        BiquadCoefficients::high_pass(sample_rate, 95.0, 0.707)?,
        BiquadCoefficients::high_pass(sample_rate, 95.0, 0.707)?,
        BiquadCoefficients::low_pass(sample_rate, 15_000.0, 0.707)?,
        BiquadCoefficients::peaking(sample_rate, 3_200.0, 1.0, 4.0)?,
    ])
}

impl Channel for AcousticRerecord {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::SimulatedAcoustic
    }

    fn is_simulated_physical_path(&self) -> bool {
        true
    }

    fn params(&self) -> Params {
        Params::from([
            param("rt60_seconds", self.preset.rt60_seconds),
            param("source_distance_m", self.preset.source_distance_m),
            param("early_reflections", self.preset.early_reflections),
            param("room_noise_snr_db", self.preset.room_noise_snr_db),
            param("clock_drift", self.preset.clock_drift),
            param(
                "direct_to_reverberant_db",
                self.preset.direct_to_reverberant_db,
            ),
            param("transducer_curve", "hp95x2_lp15k_peak3k2_plus4db"),
            param("level_matched_to_input_rms", true),
            param("simulated", true),
            param("disclaimer", SIMULATION_DISCLAIMER),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let rate = audio.sample_rate();
        let ir = synthesise_rir(self.preset, rate, seed)?;
        let stages = transducer_chain(rate)?;
        let source_rms = mean_square(audio).sqrt();

        let mut planes: Vec<Vec<f32>> = Vec::with_capacity(audio.channels());
        for channel in 0..audio.channels() {
            let plane = audio.channel(channel).ok_or(ChannelError::InputTooShort {
                frames: audio.frames(),
                needed: 1,
            })?;
            let mut wet = convolve(plane, &ir).map_err(AudioError::from)?;
            wet.truncate(plane.len());
            cascade(&stages, &mut wet);
            planes.push(wet);
        }

        let shaped = from_channels(rate, &planes)?;
        let mut captured = crate::audio::admit(
            resample_ratio(&shaped, 1.0 / self.preset.clock_drift).map_err(AudioError::from)?,
        )?;
        let captured_rms = mean_square(&captured).sqrt();
        if captured_rms > 1e-12 && source_rms > 1e-12 {
            let factor = (source_rms / captured_rms) as f32;
            captured = map_samples(&captured, |s| s * factor)?;
        }

        let power = mean_square(&captured);
        if power <= 0.0 {
            return Ok(captured);
        }
        let sigma = (power / 10f64.powf(self.preset.room_noise_snr_db / 10.0)).sqrt() as f32;
        let mut rng = Rng::new(seed ^ 0x4e4f_4953_455f_4d49);
        map_samples(&captured, |s| rng.next_gaussian().mul_add(sigma, s))
            .map_err(ChannelError::from)
    }
}
