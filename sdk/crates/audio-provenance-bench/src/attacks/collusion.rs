use crate::audio::from_channels;
use crate::error::{AudioError, ChannelError};
use audio_provenance_audio::AudioBuffer;

fn same_shape(copies: &[AudioBuffer]) -> Result<(), ChannelError> {
    let Some(first) = copies.first() else {
        return Err(ChannelError::Parameter {
            parameter: "collusion_copies",
            reason: "at least one copy is needed".to_owned(),
        });
    };
    for other in copies.iter().skip(1) {
        if other.sample_rate() != first.sample_rate()
            || other.channels() != first.channels()
            || other.frames() != first.frames()
        {
            return Err(ChannelError::Parameter {
                parameter: "collusion_copies",
                reason: "every copy must share rate, channel count and length".to_owned(),
            });
        }
    }
    Ok(())
}

/// The sample-wise mean of several marked copies of one recording.
///
/// This is the averaging attack. Its whole premise is that the copies differ only in their marks,
/// so the mean keeps the content and averages the marks toward their common part.
pub fn average(copies: &[AudioBuffer]) -> Result<AudioBuffer, ChannelError> {
    same_shape(copies)?;
    let first = copies.first().ok_or(AudioError::Empty)?;
    let scale = 1.0f32 / copies.len() as f32;
    let mut planes = vec![vec![0.0f32; first.frames()]; first.channels()];
    for copy in copies {
        for (channel, plane) in planes.iter_mut().enumerate() {
            let source = copy.channel(channel).ok_or(AudioError::Empty)?;
            for (slot, sample) in plane.iter_mut().zip(source.iter()) {
                *slot += *sample;
            }
        }
    }
    for plane in &mut planes {
        for sample in plane.iter_mut() {
            *sample *= scale;
        }
    }
    Ok(from_channels(first.sample_rate(), &planes)?)
}

/// Subtracts `strength` times the difference between a marked copy and a colluded estimate of the
/// unmarked content. At strength 1 the result is the estimate itself; above 1 it overshoots, which
/// is what an attacker does when the estimate is known to be an underestimate.
pub fn remove_estimated_residual(
    marked: &AudioBuffer,
    estimate: &AudioBuffer,
    strength: f64,
) -> Result<AudioBuffer, ChannelError> {
    same_shape(&[marked.clone(), estimate.clone()])?;
    let strength = strength as f32;
    let mut planes = Vec::with_capacity(marked.channels());
    for channel in 0..marked.channels() {
        let source = marked.channel(channel).ok_or(AudioError::Empty)?;
        let reference = estimate.channel(channel).ok_or(AudioError::Empty)?;
        planes.push(
            source
                .iter()
                .zip(reference.iter())
                .map(|(sample, other)| sample - strength * (sample - other))
                .collect::<Vec<f32>>(),
        );
    }
    Ok(from_channels(marked.sample_rate(), &planes)?)
}
