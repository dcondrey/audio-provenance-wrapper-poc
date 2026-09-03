use super::Channel;
use super::acoustic::{AcousticRerecord, RoomPreset};
use super::chain::Chain;
use super::codec::{DEFAULT_FFMPEG, LossyCodec, LossyFormat};
use super::native::{
    AdditiveNoise, Compressor, Crop, Gain, Identity, Limiter, Lowpass, NormalizePeak,
    PlaybackRateDrift, Requantize, ResampleVia,
};

/// The channel matrix the bench ships with. Every entry appears in the report; a channel whose
/// external program is missing becomes an error row rather than disappearing.
pub fn default_matrix(ffmpeg: &str) -> Vec<Box<dyn Channel>> {
    let mut matrix: Vec<Box<dyn Channel>> = vec![Box::new(Identity)];

    for kbps in [320u32, 192, 128, 96, 64] {
        matrix.push(Box::new(LossyCodec::new(
            format!("mp3_{kbps}"),
            LossyFormat::Mp3,
            kbps,
            ffmpeg,
        )));
    }
    for kbps in [256u32, 128, 64] {
        matrix.push(Box::new(LossyCodec::new(
            format!("aac_{kbps}"),
            LossyFormat::AacLc,
            kbps,
            ffmpeg,
        )));
    }
    for kbps in [128u32, 64] {
        matrix.push(Box::new(LossyCodec::new(
            format!("opus_{kbps}"),
            LossyFormat::Opus,
            kbps,
            ffmpeg,
        )));
    }

    matrix.push(Box::new(ResampleVia::new("resample_via_44100", 44_100)));
    matrix.push(Box::new(ResampleVia::new("resample_via_22050", 22_050)));

    matrix.push(Box::new(Requantize::new("requantize_16bit", 16)));
    matrix.push(Box::new(Requantize::new("requantize_8bit", 8)));

    matrix.push(Box::new(Gain::new("gain_minus_12db", -12.0)));
    matrix.push(Box::new(Gain::new("gain_minus_6db", -6.0)));
    matrix.push(Box::new(Gain::new("gain_plus_6db", 6.0)));
    matrix.push(Box::new(NormalizePeak::new(-0.1)));

    matrix.push(Box::new(Crop::new("crop_0s5", 0.5)));
    matrix.push(Box::new(Crop::new("crop_2s", 2.0)));
    matrix.push(Box::new(Crop::new("crop_7s3", 7.3)));

    matrix.push(Box::new(PlaybackRateDrift::new("drift_plus_0p1pct", 1.001)));
    matrix.push(Box::new(PlaybackRateDrift::new(
        "drift_minus_0p1pct",
        0.999,
    )));

    matrix.push(Box::new(AdditiveNoise::new("noise_snr_40db", 40.0)));
    matrix.push(Box::new(AdditiveNoise::new("noise_snr_30db", 30.0)));
    matrix.push(Box::new(AdditiveNoise::new("noise_snr_20db", 20.0)));

    matrix.push(Box::new(Lowpass::new("lowpass_16k", 16_000.0)));
    matrix.push(Box::new(Lowpass::new("lowpass_11k", 11_000.0)));

    matrix.push(Box::new(Compressor::mastering_default()));
    matrix.push(Box::new(Limiter::brickwall_default()));

    matrix.push(Box::new(AcousticRerecord::new(
        "acoustic_small_room",
        RoomPreset::SMALL,
    )));
    matrix.push(Box::new(AcousticRerecord::new(
        "acoustic_medium_room",
        RoomPreset::MEDIUM,
    )));
    matrix.push(Box::new(AcousticRerecord::new(
        "acoustic_large_room",
        RoomPreset::LARGE,
    )));

    matrix.push(Box::new(Chain::new(
        "chain_transcode_mp3_128_aac_128",
        vec![
            Box::new(LossyCodec::new("mp3_128", LossyFormat::Mp3, 128, ffmpeg)),
            Box::new(LossyCodec::new("aac_128", LossyFormat::AacLc, 128, ffmpeg)),
        ],
    )));
    matrix.push(Box::new(Chain::new(
        "chain_broadcast_limit_mp3_192",
        vec![
            Box::new(Limiter::brickwall_default()),
            Box::new(LossyCodec::new("mp3_192", LossyFormat::Mp3, 192, ffmpeg)),
        ],
    )));
    matrix.push(Box::new(Chain::new(
        "chain_worst_case_limit_mp3_128_acoustic_mp3_192",
        vec![
            Box::new(Limiter::brickwall_default()),
            Box::new(LossyCodec::new("mp3_128", LossyFormat::Mp3, 128, ffmpeg)),
            Box::new(AcousticRerecord::new(
                "acoustic_medium_room",
                RoomPreset::MEDIUM,
            )),
            Box::new(LossyCodec::new("mp3_192", LossyFormat::Mp3, 192, ffmpeg)),
        ],
    )));

    matrix
}

pub fn default_matrix_with_system_ffmpeg() -> Vec<Box<dyn Channel>> {
    default_matrix(DEFAULT_FFMPEG)
}
