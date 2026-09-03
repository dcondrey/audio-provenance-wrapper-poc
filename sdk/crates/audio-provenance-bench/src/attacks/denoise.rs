use super::statistic::percentile;
use crate::audio::from_channels;
use crate::error::{AudioError, ChannelError};
use audio_provenance_audio::window::{self, Symmetry};
use audio_provenance_audio::{AudioBuffer, Stft};

/// Stationary-noise spectral subtraction, the off-the-shelf restoration tool an attacker reaches
/// for first.
///
/// The per-bin floor is a low percentile of that bin's magnitude over the whole file, which is the
/// standard stationary estimate. `over_subtraction` multiplies that floor before it is removed and
/// `spectral_floor` is the fraction of the original magnitude that always survives.
pub fn spectral_subtraction(
    audio: &AudioBuffer,
    frame: usize,
    over_subtraction: f64,
    spectral_floor: f64,
    noise_percentile: f64,
) -> Result<AudioBuffer, ChannelError> {
    if frame < 64 || !frame.is_multiple_of(2) {
        return Err(ChannelError::Parameter {
            parameter: "denoise_frame",
            reason: "frame must be even and at least 64".to_owned(),
        });
    }
    if !(0.0..=1.0).contains(&spectral_floor) || !(0.0..1.0).contains(&noise_percentile) {
        return Err(ChannelError::Parameter {
            parameter: "denoise_thresholds",
            reason: "spectral floor must be in 0..=1 and the noise percentile in 0..1".to_owned(),
        });
    }
    let win: Vec<f32> = window::hann(frame, Symmetry::Periodic)
        .into_iter()
        .map(|value| value.max(0.0).sqrt())
        .collect();
    let stft = Stft::new(win, frame / 2).map_err(AudioError::Substrate)?;
    let floor = spectral_floor as f32;
    let mut planes = Vec::with_capacity(audio.channels());
    for channel in 0..audio.channels() {
        let plane = audio.channel(channel).ok_or(AudioError::Empty)?;
        let mut spectra = stft.forward(plane).map_err(AudioError::Substrate)?;
        let bins = spectra.bins();
        let frames = spectra.frames();
        let mut noise = vec![0.0f32; bins];
        let mut column = Vec::with_capacity(frames);
        for (bin, slot) in noise.iter_mut().enumerate().take(bins) {
            column.clear();
            for index in 0..frames {
                if let Some(spectrum) = spectra.frame(index)
                    && let Some(value) = spectrum.get(bin)
                {
                    column.push(f64::from(value.norm_sqr()).sqrt());
                }
            }
            *slot = percentile(&mut column, noise_percentile).unwrap_or(0.0) as f32;
        }
        for index in 0..frames {
            let Some(spectrum) = spectra.frame_mut(index) else {
                continue;
            };
            for (bin, value) in spectrum.iter_mut().enumerate() {
                let magnitude = f64::from(value.norm_sqr()).sqrt() as f32;
                if magnitude <= f32::MIN_POSITIVE {
                    continue;
                }
                let reduced =
                    (magnitude - over_subtraction as f32 * noise[bin]).max(floor * magnitude);
                *value *= reduced / magnitude;
            }
        }
        planes.push(
            stft.inverse(&spectra, audio.frames())
                .map_err(AudioError::Substrate)?,
        );
    }
    Ok(from_channels(audio.sample_rate(), &planes)?)
}
