use crate::audio::{from_channels, from_f32le_bytes, to_f32le_bytes};
use crate::dsp::Rng;
use crate::error::{AudioError, ChannelError, PortError};
use crate::ports::CommandRunner;
use audio_provenance_audio::AudioBuffer;
use audio_provenance_audio::resample::resample_ratio;

/// Playback faster or slower by `fraction`, time and pitch moving together, which is what a
/// free-running clock or a deliberate speed change does.
pub fn playback_rate(audio: &AudioBuffer, fraction: f64) -> Result<AudioBuffer, ChannelError> {
    if !fraction.is_finite() || fraction <= -1.0 {
        return Err(ChannelError::Parameter {
            parameter: "playback_rate_fraction",
            reason: "must be finite and greater than -1".to_owned(),
        });
    }
    Ok(resample_ratio(audio, 1.0 / (1.0 + fraction)).map_err(AudioError::Substrate)?)
}

fn run_filter(
    audio: &AudioBuffer,
    program: &str,
    filter: &str,
    runner: &dyn CommandRunner,
) -> Result<AudioBuffer, ChannelError> {
    let rate = audio.sample_rate().to_string();
    let channels = audio.channels().to_string();
    let args: Vec<String> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-f",
        "f32le",
        "-ar",
        &rate,
        "-ac",
        &channels,
        "-i",
        "pipe:0",
        "-af",
        filter,
        "-f",
        "f32le",
        "-ar",
        &rate,
        "-ac",
        &channels,
        "pipe:1",
    ]
    .iter()
    .map(|value| (*value).to_owned())
    .collect();
    let output = runner.run(program, &args, &to_f32le_bytes(audio))?;
    if output.status != 0 || output.stdout.is_empty() {
        return Err(ChannelError::Port(PortError::Status {
            program: program.to_owned(),
            status: output.status,
            stderr: if output.stderr.is_empty() {
                "filter produced no output".to_owned()
            } else {
                output.stderr
            },
        }));
    }
    Ok(from_f32le_bytes(
        &output.stdout,
        audio.sample_rate(),
        audio.channels(),
    )?)
}

/// Pitch-preserving tempo change. `fraction` is the speed-up, so 0.01 plays one percent faster at
/// the same pitch.
pub fn time_stretch(
    audio: &AudioBuffer,
    fraction: f64,
    program: &str,
    runner: &dyn CommandRunner,
) -> Result<AudioBuffer, ChannelError> {
    let tempo = 1.0 + fraction;
    if !(0.5..=2.0).contains(&tempo) {
        return Err(ChannelError::Parameter {
            parameter: "time_stretch_fraction",
            reason: "atempo accepts 0.5 to 2.0 in one stage".to_owned(),
        });
    }
    run_filter(audio, program, &format!("atempo={tempo:.9}"), runner)
}

/// Tempo-preserving pitch shift, in cents.
pub fn pitch_shift(
    audio: &AudioBuffer,
    cents: f64,
    program: &str,
    runner: &dyn CommandRunner,
) -> Result<AudioBuffer, ChannelError> {
    let ratio = 2f64.powf(cents / 1200.0);
    if !(0.5..=2.0).contains(&ratio) {
        return Err(ChannelError::Parameter {
            parameter: "pitch_shift_cents",
            reason: "the compensating atempo stage accepts 0.5 to 2.0".to_owned(),
        });
    }
    let rate = f64::from(audio.sample_rate()) * ratio;
    let filter = format!(
        "asetrate={rate:.6},aresample={},atempo={:.9}",
        audio.sample_rate(),
        1.0 / ratio
    );
    run_filter(audio, program, &filter, runner)
}

/// Deletes one frame out of every `period`, the cheapest desynchronisation there is.
pub fn drop_samples(audio: &AudioBuffer, period: usize) -> Result<AudioBuffer, ChannelError> {
    if period < 2 {
        return Err(ChannelError::Parameter {
            parameter: "drop_period",
            reason: "must drop at most one frame in two".to_owned(),
        });
    }
    let mut planes = Vec::with_capacity(audio.channels());
    for channel in 0..audio.channels() {
        let plane = audio.channel(channel).ok_or(AudioError::Empty)?;
        planes.push(
            plane
                .iter()
                .enumerate()
                .filter_map(|(index, sample)| (!index.is_multiple_of(period)).then_some(*sample))
                .collect::<Vec<f32>>(),
        );
    }
    Ok(from_channels(audio.sample_rate(), &planes)?)
}

/// Independently resamples consecutive segments by small random ratios and concatenates them, so
/// the timing error accumulates and reverses rather than holding one constant rate.
pub fn random_warp(
    audio: &AudioBuffer,
    max_fraction: f64,
    segment_seconds: f64,
    seed: u64,
) -> Result<AudioBuffer, ChannelError> {
    if !(max_fraction.is_finite() && max_fraction > 0.0 && max_fraction < 0.5) {
        return Err(ChannelError::Parameter {
            parameter: "warp_max_fraction",
            reason: "must be in (0, 0.5)".to_owned(),
        });
    }
    let segment = (segment_seconds * f64::from(audio.sample_rate())) as usize;
    if segment < 1024 {
        return Err(ChannelError::Parameter {
            parameter: "warp_segment_seconds",
            reason: "segment must hold at least 1024 frames".to_owned(),
        });
    }
    let mut rng = Rng::new(seed);
    let mut planes: Vec<Vec<f32>> = vec![Vec::with_capacity(audio.frames()); audio.channels()];
    let mut start = 0usize;
    while start < audio.frames() {
        let end = (start + segment).min(audio.frames());
        let mut slice = Vec::with_capacity(audio.channels());
        for channel in 0..audio.channels() {
            let plane = audio.channel(channel).ok_or(AudioError::Empty)?;
            slice.push(plane[start..end].to_vec());
        }
        let piece = from_channels(audio.sample_rate(), &slice)?;
        let fraction = f64::from(rng.next_symmetric()) * max_fraction;
        let warped =
            resample_ratio(&piece, 1.0 / (1.0 + fraction)).map_err(AudioError::Substrate)?;
        for (channel, plane) in planes.iter_mut().enumerate() {
            let source = warped.channel(channel).ok_or(AudioError::Empty)?;
            plane.extend_from_slice(source);
        }
        start = end;
    }
    Ok(from_channels(audio.sample_rate(), &planes)?)
}
