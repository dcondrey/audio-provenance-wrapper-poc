use audio_provenance_core::CodedError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GenoMarkError {
    #[error("sample rate {found} is below the {min} Hz floor; the band top would be clipped away")]
    SampleRateTooLow { found: u32, min: u32 },
    #[error("sample rate {found} is above the {max} Hz ceiling this build embeds at")]
    SampleRateTooHigh { found: u32, max: u32 },
    #[error("audio holds no frames")]
    Empty,
    #[error("audio holds {frames} frames, fewer than the {needed} one block needs")]
    TooShort { frames: usize, needed: usize },
    #[error("audio holds {frames} frames across {channels} channels, over the {limit} sample cap")]
    TooLarge {
        frames: usize,
        channels: usize,
        limit: usize,
    },
    #[error("sample at channel {channel}, frame {frame} is not finite")]
    NonFinite { channel: usize, frame: usize },
    #[error("payload is {found} bytes; the 56-bit GenoMark payload is exactly {expected}")]
    PayloadLength { found: usize, expected: usize },
    #[error("payload field `{field}` holds {found}, over the {max} its {bits}-bit width allows")]
    PayloadFieldRange {
        field: &'static str,
        found: u64,
        max: u64,
        bits: u32,
    },
    #[error(
        "cell {cell} spans no bins at {sample_rate} Hz with a {frame}-sample frame and rate factor \
         {rho}"
    )]
    DegenerateCell {
        cell: usize,
        sample_rate: u32,
        frame: usize,
        rho: f64,
    },
    #[error("band top bin {top} does not fit in the {bins} bins of a {frame}-sample frame")]
    BandOutOfRange {
        top: usize,
        bins: usize,
        frame: usize,
    },
    #[error(
        "guard mask left {found} active pairs, not the {expected} the rate budget is derived from"
    )]
    GuardMaskShape { found: usize, expected: usize },
    #[error("profile key is empty; a keyed namespace needs key material")]
    EmptyProfileKey,
    #[error("key derivation rejected a {bytes} byte expansion")]
    KeyDerivation { bytes: usize },
    #[error("two-pass closure left {residual} nepers on slot {slot}, over the {limit} budget")]
    ClosureResidual {
        slot: usize,
        residual: f64,
        limit: f64,
    },
    #[error(transparent)]
    Audio(#[from] audio_provenance_audio::AudioError),
}

impl CodedError for GenoMarkError {
    fn code(&self) -> &'static str {
        match self {
            Self::SampleRateTooLow { .. } => "fs_too_low",
            Self::SampleRateTooHigh { .. } => "fs_too_high",
            Self::Empty => "audio_empty",
            Self::TooShort { .. } => "audio_too_short",
            Self::TooLarge { .. } => "audio_too_large",
            Self::NonFinite { .. } => "audio_non_finite_sample",
            Self::PayloadLength { .. } => "payload_length_invalid",
            Self::PayloadFieldRange { .. } => "payload_field_out_of_range",
            Self::DegenerateCell { .. } => "cell_geometry_degenerate",
            Self::BandOutOfRange { .. } => "band_out_of_range",
            Self::GuardMaskShape { .. } => "guard_mask_shape_unexpected",
            Self::EmptyProfileKey => "profile_key_empty",
            Self::KeyDerivation { .. } => "key_derivation_failed",
            Self::ClosureResidual { .. } => "ola_closure_residual",
            Self::Audio(inner) => inner.code(),
        }
    }
}
