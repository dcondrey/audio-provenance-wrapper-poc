use audio_provenance_audio::AudioBuffer;

use crate::budget::MaskingModel;
use crate::error::GenoMarkNError;
use crate::params::ANALYSIS_SAMPLE_RATE;
use crate::payload::Payload;
use crate::session::EncoderSession;
use crate::spectral::{Analysis, at_analysis_rate};

/// Embeds the mark and returns the host with the residual added.
///
/// THE HOST IS NEVER RESAMPLED (spec 2.2). A copy goes to 48 kHz, the encoder runs there, the
/// residual is formed at 48 kHz and resampled back, and the host's own samples outside the
/// residual's support survive bit-for-bit. The residual is band-limited to 7.7 kHz by construction,
/// so a polyphase conversion is transparent in band.
///
/// The mask DIRECTION is read once from the mono sum and applied to every channel, so a downmix
/// carries the mark coherently instead of two independently-drawn residuals that partially cancel.
/// The mask MAGNITUDE is the per-channel perceptual budget, so a quiet channel is not handed a
/// loud channel's headroom.
pub fn embed(
    encoder: &EncoderSession,
    analysis: &Analysis,
    masking: &MaskingModel,
    host: &AudioBuffer,
    payload: Payload,
) -> Result<AudioBuffer, GenoMarkNError> {
    let work = at_analysis_rate(host)?;
    let mono = work.mono_sum();
    let frames = analysis.forward(&mono)?;
    let time = frames.frames();
    let log_mag = analysis.band_log_magnitude(&frames);
    let direction = encoder.run(&log_mag, time, &payload.to_message_bits())?;

    let mut residual = AudioBuffer::silence(ANALYSIS_SAMPLE_RATE, work.channels(), work.frames())?;
    for channel in 0..work.channels() {
        let Some(plane) = work.channel(channel) else {
            continue;
        };
        let mut spectrum = analysis.forward(plane)?;
        let budget = masking.budget(&spectrum);
        let gain: Vec<f32> = direction
            .iter()
            .zip(budget.iter())
            .map(|(raw, ceiling)| raw.clamp(-1.0, 1.0) * ceiling)
            .collect();
        analysis.apply_log_gain(&mut spectrum, &gain);
        let marked = analysis.inverse(&spectrum, plane.len())?;
        let Some(slot) = residual.channel_mut(channel) else {
            continue;
        };
        for ((out, cover), test) in slot.iter_mut().zip(plane.iter()).zip(marked.iter()) {
            *out = test - cover;
        }
    }

    let native = if residual.sample_rate() == host.sample_rate() {
        residual
    } else {
        audio_provenance_audio::resample(&residual, host.sample_rate())?
    };

    let mut out = host.clone();
    for channel in 0..out.channels() {
        let Some(added) = native.channel(channel).map(<[f32]>::to_vec) else {
            continue;
        };
        let Some(slot) = out.channel_mut(channel) else {
            continue;
        };
        for (sample, delta) in slot.iter_mut().zip(added.iter()) {
            *sample += delta;
        }
    }
    Ok(out)
}
