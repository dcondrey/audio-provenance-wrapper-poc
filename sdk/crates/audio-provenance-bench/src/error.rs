use audio_provenance_core::CodedError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PortError {
    #[error("external program `{program}` could not be started: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("external program `{program}` exited with status {status}: {stderr}")]
    Status {
        program: String,
        status: i32,
        stderr: String,
    },
    #[error("external program `{program}` produced {bytes} bytes, over the {limit} byte cap")]
    OutputTooLarge {
        program: String,
        bytes: usize,
        limit: usize,
    },
    #[error("i/o error on `{path}`: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("this build has no implementation for external program `{program}`")]
    Unavailable { program: String },
}

impl CodedError for PortError {
    fn code(&self) -> &'static str {
        match self {
            Self::Spawn { .. } => "port_spawn_failed",
            Self::Status { .. } => "port_nonzero_status",
            Self::OutputTooLarge { .. } => "port_output_too_large",
            Self::Io { .. } => "port_io_failed",
            Self::Unavailable { .. } => "port_unavailable",
        }
    }
}

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("audio buffer holds no frames")]
    Empty,
    #[error("sample rate {found} is outside the supported range {min}..={max}")]
    SampleRate { found: u32, min: u32, max: u32 },
    #[error("channel count {found} is outside the supported range 1..={max}")]
    ChannelCount { found: usize, max: usize },
    #[error("{len} interleaved samples is not a whole number of {channels}-channel frames")]
    Ragged { len: usize, channels: usize },
    #[error("sample at channel {channel}, frame {frame} is not finite")]
    NonFinite { channel: usize, frame: usize },
    #[error("{frames} frames x {channels} channels exceeds the {limit} sample buffer cap")]
    TooLarge {
        frames: usize,
        channels: usize,
        limit: usize,
    },
    #[error("cannot compare: {what} differs, original {original} against marked {marked}")]
    Mismatch {
        what: &'static str,
        original: u64,
        marked: u64,
    },
    #[error(transparent)]
    Substrate(#[from] audio_provenance_audio::AudioError),
}

impl CodedError for AudioError {
    fn code(&self) -> &'static str {
        match self {
            Self::Empty => "audio_empty",
            Self::SampleRate { .. } => "audio_sample_rate_unsupported",
            Self::ChannelCount { .. } => "audio_channel_count_unsupported",
            Self::Ragged { .. } => "audio_ragged_frames",
            Self::NonFinite { .. } => "audio_non_finite_sample",
            Self::TooLarge { .. } => "audio_buffer_too_large",
            Self::Mismatch { .. } => "audio_comparison_mismatch",
            Self::Substrate(inner) => inner.code(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error(transparent)]
    Port(#[from] PortError),
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error("channel parameter `{parameter}` is invalid: {reason}")]
    Parameter {
        parameter: &'static str,
        reason: String,
    },
    #[error("input is {frames} frames, shorter than the {needed} this channel consumes")]
    InputTooShort { frames: usize, needed: usize },
    #[error("decoded stream is {bytes} bytes, not a whole number of {channels}-channel f32 frames")]
    RaggedDecode { bytes: usize, channels: usize },
}

impl CodedError for ChannelError {
    fn code(&self) -> &'static str {
        match self {
            Self::Port(inner) => inner.code(),
            Self::Audio(inner) => inner.code(),
            Self::Parameter { .. } => "channel_parameter_invalid",
            Self::InputTooShort { .. } => "channel_input_too_short",
            Self::RaggedDecode { .. } => "channel_ragged_decode",
        }
    }
}

#[derive(Debug, Error)]
pub enum CodecError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error("payload is {found} bytes, but this codec carries exactly {expected}")]
    PayloadLength { found: usize, expected: usize },
    #[error("audio holds {frames} frames, fewer than the {needed} this codec needs")]
    TooShort { frames: usize, needed: usize },
}

impl CodedError for CodecError {
    fn code(&self) -> &'static str {
        match self {
            Self::Audio(inner) => inner.code(),
            Self::PayloadLength { .. } => "codec_payload_length_invalid",
            Self::TooShort { .. } => "codec_audio_too_short",
        }
    }
}

#[derive(Debug, Error)]
pub enum BenchError {
    #[error(transparent)]
    Audio(#[from] AudioError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error(transparent)]
    Port(#[from] PortError),
    #[error("the corpus is empty; nothing can be measured")]
    EmptyCorpus,
    #[error("the channel matrix is empty; nothing can be measured")]
    EmptyMatrix,
    #[error("report serialisation failed: {source}")]
    Serialisation {
        #[source]
        source: serde_json::Error,
    },
    #[error("threshold for channel `{channel}` is invalid: {reason}")]
    Threshold { channel: String, reason: String },
}

impl CodedError for BenchError {
    fn code(&self) -> &'static str {
        match self {
            Self::Audio(inner) => inner.code(),
            Self::Channel(inner) => inner.code(),
            Self::Codec(inner) => inner.code(),
            Self::Port(inner) => inner.code(),
            Self::EmptyCorpus => "bench_corpus_empty",
            Self::EmptyMatrix => "bench_matrix_empty",
            Self::Serialisation { .. } => "bench_serialisation_failed",
            Self::Threshold { .. } => "bench_threshold_invalid",
        }
    }
}
