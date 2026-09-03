use super::{Channel, ChannelFamily, Params, param};
use crate::audio::{from_planes, map_samples, mean_square, peak};
use crate::dsp::Rng;
use crate::error::{AudioError, ChannelError};
use crate::ports::CommandRunner;
use audio_provenance_audio::AudioBuffer;
use audio_provenance_audio::biquad::{BiquadCoefficients, cascade};
use audio_provenance_audio::resample::{resample, resample_ratio};

#[derive(Debug, Clone, Copy, Default)]
pub struct Identity;

impl Channel for Identity {
    fn name(&self) -> &str {
        "identity"
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Control
    }

    fn params(&self) -> Params {
        Params::new()
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        Ok(audio.clone())
    }
}

#[derive(Debug, Clone)]
pub struct Gain {
    name: String,
    decibels: f64,
}

impl Gain {
    pub fn new(name: impl Into<String>, decibels: f64) -> Self {
        Self {
            name: name.into(),
            decibels,
        }
    }
}

impl Channel for Gain {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Level
    }

    fn params(&self) -> Params {
        Params::from([param("gain_db", self.decibels)])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let factor = 10f32.powf((self.decibels / 20.0) as f32);
        Ok(map_samples(audio, |s| s * factor)?)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NormalizePeak {
    target_dbfs: f64,
}

impl NormalizePeak {
    pub const fn new(target_dbfs: f64) -> Self {
        Self { target_dbfs }
    }
}

impl Channel for NormalizePeak {
    fn name(&self) -> &str {
        "normalize_peak"
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Level
    }

    fn params(&self) -> Params {
        Params::from([param("target_peak_dbfs", self.target_dbfs)])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let level = peak(audio);
        if level <= f32::MIN_POSITIVE {
            return Ok(audio.clone());
        }
        let target = 10f32.powf((self.target_dbfs / 20.0) as f32);
        let factor = target / level;
        Ok(map_samples(audio, |s| s * factor)?)
    }
}

#[derive(Debug, Clone)]
pub struct Crop {
    name: String,
    drop_seconds: f64,
}

impl Crop {
    pub fn new(name: impl Into<String>, drop_seconds: f64) -> Self {
        Self {
            name: name.into(),
            drop_seconds,
        }
    }

    /// Exposed so a test can pin that 7.3 s is not a whole number of any common analysis frame.
    pub fn dropped_frames(&self, sample_rate: u32) -> usize {
        (self.drop_seconds * f64::from(sample_rate))
            .round()
            .max(0.0) as usize
    }
}

impl Channel for Crop {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Editing
    }

    fn params(&self) -> Params {
        Params::from([param("drop_leading_seconds", self.drop_seconds)])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let drop = self.dropped_frames(audio.sample_rate());
        if drop >= audio.frames() {
            return Err(ChannelError::InputTooShort {
                frames: audio.frames(),
                needed: drop + 1,
            });
        }
        let channels = audio.channels();
        let mut planes = Vec::with_capacity((audio.frames() - drop) * channels);
        for channel in 0..channels {
            let plane = audio.channel(channel).ok_or(ChannelError::InputTooShort {
                frames: audio.frames(),
                needed: drop + 1,
            })?;
            planes.extend_from_slice(&plane[drop..]);
        }
        Ok(from_planes(audio.sample_rate(), channels, planes)?)
    }
}

#[derive(Debug, Clone)]
pub struct Requantize {
    name: String,
    bits: u32,
}

impl Requantize {
    pub fn new(name: impl Into<String>, bits: u32) -> Self {
        Self {
            name: name.into(),
            bits,
        }
    }
}

impl Channel for Requantize {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Sampling
    }

    fn params(&self) -> Params {
        Params::from([param("bits", self.bits), param("dither", "none")])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        if !(2..=24).contains(&self.bits) {
            return Err(ChannelError::Parameter {
                parameter: "bits",
                reason: format!("{} is outside 2..=24", self.bits),
            });
        }
        let levels = (1u32 << (self.bits - 1)) as f32;
        Ok(map_samples(audio, |s| {
            ((s * levels).round() / levels).clamp(-1.0, 1.0)
        })?)
    }
}

#[derive(Debug, Clone)]
pub struct AdditiveNoise {
    name: String,
    snr_db: f64,
}

impl AdditiveNoise {
    pub fn new(name: impl Into<String>, snr_db: f64) -> Self {
        Self {
            name: name.into(),
            snr_db,
        }
    }
}

impl Channel for AdditiveNoise {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Noise
    }

    fn params(&self) -> Params {
        Params::from([
            param("snr_db", self.snr_db),
            param("distribution", "gaussian"),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let power = mean_square(audio);
        if power <= 0.0 {
            return Ok(audio.clone());
        }
        let sigma = (power / 10f64.powf(self.snr_db / 10.0)).sqrt() as f32;
        let mut rng = Rng::new(seed);
        Ok(map_samples(audio, |s| {
            rng.next_gaussian().mul_add(sigma, s)
        })?)
    }
}

#[derive(Debug, Clone)]
pub struct Lowpass {
    name: String,
    cutoff_hz: f64,
}

impl Lowpass {
    pub fn new(name: impl Into<String>, cutoff_hz: f64) -> Self {
        Self {
            name: name.into(),
            cutoff_hz,
        }
    }
}

impl Channel for Lowpass {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Bandwidth
    }

    fn params(&self) -> Params {
        Params::from([
            param("cutoff_hz", self.cutoff_hz),
            param("order", 4),
            param("topology", "cascaded_rbj_biquad"),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let rate = audio.sample_rate();
        let stages = [
            BiquadCoefficients::low_pass(rate, self.cutoff_hz, 0.541).map_err(AudioError::from)?,
            BiquadCoefficients::low_pass(rate, self.cutoff_hz, 1.307).map_err(AudioError::from)?,
        ];
        let mut filtered = audio.clone();
        for channel in 0..filtered.channels() {
            if let Some(plane) = filtered.channel_mut(channel) {
                cascade(&stages, plane);
            }
        }
        Ok(crate::audio::admit(filtered)?)
    }
}

#[derive(Debug, Clone)]
pub struct ResampleVia {
    name: String,
    via_rate: u32,
}

impl ResampleVia {
    pub fn new(name: impl Into<String>, via_rate: u32) -> Self {
        Self {
            name: name.into(),
            via_rate,
        }
    }
}

impl Channel for ResampleVia {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Sampling
    }

    fn params(&self) -> Params {
        Params::from([
            param("via_sample_rate", self.via_rate),
            param("kernel", "rubato_sinc_256_blackman_harris2_cubic"),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let source = audio.sample_rate();
        let down = resample(audio, self.via_rate).map_err(AudioError::from)?;
        let back = resample(&down, source).map_err(AudioError::from)?;
        Ok(crate::audio::admit(back)?)
    }
}

/// Playback-rate drift with no pitch correction, which is what an uncorrected analogue path does.
///
/// Distinct from `audio_provenance_audio::ClockDrift`, which models a capture clock with an exact frame
/// count and a cheap cubic interpolator. This row must not measure its own interpolator, so it
/// resamples band-limited instead.
#[derive(Debug, Clone)]
pub struct PlaybackRateDrift {
    name: String,
    factor: f64,
}

impl PlaybackRateDrift {
    pub fn new(name: impl Into<String>, factor: f64) -> Self {
        Self {
            name: name.into(),
            factor,
        }
    }
}

impl Channel for PlaybackRateDrift {
    fn name(&self) -> &str {
        &self.name
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Sampling
    }

    fn params(&self) -> Params {
        Params::from([
            param("playback_rate_factor", self.factor),
            param("pitch_corrected", false),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        if !(0.5..=2.0).contains(&self.factor) {
            return Err(ChannelError::Parameter {
                parameter: "playback_rate_factor",
                reason: format!("{} is outside 0.5..=2.0", self.factor),
            });
        }
        let drifted = resample_ratio(audio, 1.0 / self.factor).map_err(AudioError::from)?;
        Ok(crate::audio::admit(drifted)?)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Compressor {
    threshold_db: f64,
    ratio: f64,
    attack_ms: f64,
    release_ms: f64,
    makeup_db: f64,
}

impl Compressor {
    pub const fn mastering_default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 4.0,
            attack_ms: 5.0,
            release_ms: 120.0,
            makeup_db: 6.0,
        }
    }
}

fn coefficient(ms: f64, sample_rate: u32) -> f64 {
    if ms <= 0.0 {
        return 0.0;
    }
    (-1.0 / (ms * 0.001 * f64::from(sample_rate))).exp()
}

/// Peak across every channel of one frame, which is what a linked sidechain reads.
fn frame_peak(planes: &[f32], channels: usize, frames: usize, frame: usize) -> f32 {
    (0..channels).fold(0.0f32, |acc, channel| {
        planes
            .get(channel * frames + frame)
            .map_or(acc, |sample| acc.max(sample.abs()))
    })
}

impl Channel for Compressor {
    fn name(&self) -> &str {
        "compress_dynamic"
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Dynamics
    }

    fn params(&self) -> Params {
        Params::from([
            param("threshold_db", self.threshold_db),
            param("ratio", self.ratio),
            param("attack_ms", self.attack_ms),
            param("release_ms", self.release_ms),
            param("makeup_db", self.makeup_db),
            param("sidechain", "linked_peak_across_channels"),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        if self.ratio < 1.0 {
            return Err(ChannelError::Parameter {
                parameter: "ratio",
                reason: format!("{} is below 1.0", self.ratio),
            });
        }
        let channels = audio.channels();
        let frames = audio.frames();
        let planes = audio.planes();
        let attack = coefficient(self.attack_ms, audio.sample_rate());
        let release = coefficient(self.release_ms, audio.sample_rate());
        let makeup = 10f64.powf(self.makeup_db / 20.0);
        let mut envelope_db = -120.0f64;
        let mut out = vec![0.0f32; planes.len()];

        for frame in 0..frames {
            let level_db = 20.0
                * f64::from(frame_peak(planes, channels, frames, frame))
                    .max(1e-9)
                    .log10();
            let coeff = if level_db > envelope_db {
                attack
            } else {
                release
            };
            envelope_db = coeff.mul_add(envelope_db - level_db, level_db);
            let over = envelope_db - self.threshold_db;
            let reduction_db = if over > 0.0 {
                over * (1.0 / self.ratio - 1.0)
            } else {
                0.0
            };
            let gain = (10f64.powf(reduction_db / 20.0) * makeup) as f32;
            for channel in 0..channels {
                let at = channel * frames + frame;
                out[at] = planes[at] * gain;
            }
        }
        Ok(from_planes(audio.sample_rate(), channels, out)?)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limiter {
    ceiling_dbfs: f64,
    lookahead_ms: f64,
    release_ms: f64,
    input_gain_db: f64,
}

impl Limiter {
    pub const fn brickwall_default() -> Self {
        Self {
            ceiling_dbfs: -0.3,
            lookahead_ms: 2.0,
            release_ms: 50.0,
            input_gain_db: 6.0,
        }
    }
}

impl Channel for Limiter {
    fn name(&self) -> &str {
        "limit_brickwall"
    }

    fn family(&self) -> ChannelFamily {
        ChannelFamily::Dynamics
    }

    fn params(&self) -> Params {
        Params::from([
            param("ceiling_dbfs", self.ceiling_dbfs),
            param("lookahead_ms", self.lookahead_ms),
            param("release_ms", self.release_ms),
            param("input_gain_db", self.input_gain_db),
        ])
    }

    fn apply(
        &self,
        audio: &AudioBuffer,
        _seed: u64,
        _runner: &dyn CommandRunner,
    ) -> Result<AudioBuffer, ChannelError> {
        let channels = audio.channels();
        let frames = audio.frames();
        let lookahead = ((self.lookahead_ms * 0.001 * f64::from(audio.sample_rate())).round()
            as usize)
            .clamp(1, frames.max(1));
        let ceiling = 10f32.powf((self.ceiling_dbfs / 20.0) as f32);
        let drive = 10f32.powf((self.input_gain_db / 20.0) as f32);
        let release = coefficient(self.release_ms, audio.sample_rate()) as f32;

        let driven: Vec<f32> = audio.planes().iter().map(|&s| s * drive).collect();
        let mut target = Vec::with_capacity(frames);
        for frame in 0..frames {
            let end = (frame + lookahead).min(frames);
            let mut window_peak = 0.0f32;
            for at in frame..end {
                window_peak = window_peak.max(frame_peak(&driven, channels, frames, at));
            }
            target.push(if window_peak > ceiling {
                ceiling / window_peak
            } else {
                1.0
            });
        }

        let mut gain = 1.0f32;
        let mut out = vec![0.0f32; driven.len()];
        for (frame, wanted) in target.iter().enumerate() {
            gain = if *wanted < gain {
                *wanted
            } else {
                release.mul_add(gain - *wanted, *wanted)
            };
            for channel in 0..channels {
                let at = channel * frames + frame;
                out[at] = (driven[at] * gain).clamp(-ceiling, ceiling);
            }
        }
        Ok(from_planes(audio.sample_rate(), channels, out)?)
    }
}
