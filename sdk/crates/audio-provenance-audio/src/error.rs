use core::fmt;

use audio_provenance_core::CodedError;
use thiserror::Error;

/// A four-byte IFF/RIFF chunk identifier read from untrusted input.
///
/// IMPORTANT: `Display` escapes every non-printable byte; chunk ids reach error
/// messages and logs straight from the file being parsed.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkId(pub [u8; 4]);

impl ChunkId {
    pub const FMT: Self = Self(*b"fmt ");
    pub const DATA: Self = Self(*b"data");
    pub const COMM: Self = Self(*b"COMM");
    pub const SSND: Self = Self(*b"SSND");
}

impl fmt::Display for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            if byte.is_ascii_graphic() || byte == b' ' {
                write!(f, "{}", byte as char)?;
            } else {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for ChunkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkId(\"{self}\")")
    }
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("unsupported container: {0}")]
    UnsupportedContainer(&'static str),

    #[error("malformed {container} header: {reason}")]
    MalformedHeader {
        container: &'static str,
        reason: &'static str,
    },

    #[error("chunk {chunk} declares {declared} bytes but only {available} remain")]
    TruncatedChunk {
        chunk: ChunkId,
        declared: u64,
        available: u64,
    },

    #[error("required chunk {chunk} is missing")]
    MissingChunk { chunk: ChunkId },

    #[error("unsupported sample format: tag 0x{format_tag:04x} at {bits} bits")]
    UnsupportedSampleFormat { format_tag: u16, bits: u16 },

    #[error("invalid channel count {found}")]
    InvalidChannelCount { found: u64 },

    #[error("invalid sample rate {found}")]
    InvalidSampleRate { found: u64 },

    #[error("{requested} samples exceeds the configured limit of {limit}")]
    SampleLimitExceeded { limit: usize, requested: u64 },

    #[error("{found} bytes exceeds the configured limit of {limit}")]
    ByteLimitExceeded { limit: usize, found: usize },

    #[error("expected {expected} samples, got {found}")]
    LengthMismatch { expected: usize, found: usize },

    #[error("transform length {found} is invalid: {reason}")]
    InvalidTransformLength { found: usize, reason: &'static str },

    #[error("invalid filter parameter: {0}")]
    InvalidFilterParameter(&'static str),

    #[error("invalid resample ratio: {0}")]
    InvalidResampleRatio(&'static str),

    #[error("fft failed: {0}")]
    Fft(&'static str),

    #[error("resampler construction failed: {0}")]
    ResamplerConstruction(#[from] rubato::ResamplerConstructionError),

    #[error("resampling failed: {0}")]
    Resample(#[from] rubato::ResampleError),

    #[error("wav encoding failed: {0}")]
    Encode(#[from] hound::Error),

    #[cfg(feature = "codecs")]
    #[error("decoding failed: {0}")]
    Decode(#[from] symphonia::core::errors::Error),

    #[cfg(feature = "fs")]
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
}

impl CodedError for AudioError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedContainer(_) => "audio_container_unsupported",
            Self::MalformedHeader { .. } => "audio_header_malformed",
            Self::TruncatedChunk { .. } => "audio_chunk_truncated",
            Self::MissingChunk { .. } => "audio_chunk_missing",
            Self::UnsupportedSampleFormat { .. } => "audio_sample_format_unsupported",
            Self::InvalidChannelCount { .. } => "audio_channel_count_invalid",
            Self::InvalidSampleRate { .. } => "audio_sample_rate_invalid",
            Self::SampleLimitExceeded { .. } => "audio_sample_limit_exceeded",
            Self::ByteLimitExceeded { .. } => "audio_byte_limit_exceeded",
            Self::LengthMismatch { .. } => "audio_length_mismatch",
            Self::InvalidTransformLength { .. } => "audio_transform_length_invalid",
            Self::InvalidFilterParameter(_) => "audio_filter_parameter_invalid",
            Self::InvalidResampleRatio(_) => "audio_resample_ratio_invalid",
            Self::Fft(_) => "audio_fft_failed",
            Self::ResamplerConstruction(_) => "audio_resampler_construction_failed",
            Self::Resample(_) => "audio_resample_failed",
            Self::Encode(_) => "audio_encode_failed",
            #[cfg(feature = "codecs")]
            Self::Decode(_) => "audio_decode_failed",
            #[cfg(feature = "fs")]
            Self::Io(_) => "audio_io_failed",
        }
    }
}
