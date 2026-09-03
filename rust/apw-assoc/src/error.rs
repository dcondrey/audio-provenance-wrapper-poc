use std::path::PathBuf;

pub type Result<T> = core::result::Result<T, AssocError>;

/// Extraction failures. Every `Display` string is reproduced from the message
/// `daemon/audio_association.py` puts in the record's `reason` field, because
/// that string is copied into the signed manifest.
#[derive(Debug, thiserror::Error)]
pub enum AssocError {
    #[error("only PCM WAV and AIFF exports are supported")]
    UnsupportedSuffix,

    /// IMPORTANT: renders empty on purpose. `_open_pcm` catches only
    /// `wave.Error`, so a header or fmt chunk that runs past the end of the
    /// file raises `EOFError`, which reaches `associate_export`'s handler and
    /// is rendered by `str(exc)` as the empty string. Reproduced deliberately
    /// so the `reason` byte in the manifest matches the oracle.
    #[error("")]
    Eof,

    #[error("unsupported WAV format: {0}")]
    UnsupportedWav(&'static str),

    #[error("unsupported WAV format: unknown format: {0}")]
    UnknownWaveFormatTag(u16),

    #[error("unsupported WAV format: unknown extended format: {0}")]
    UnknownExtendedWaveFormat(String),

    #[error("invalid audio format metadata")]
    InvalidFormatMetadata,

    #[error("unsupported PCM sample width: {0} bytes")]
    UnsupportedSampleWidth(u16),

    /// AIFF has no reader here. `apw-audio` owns the AIFF chunk walk; this
    /// crate takes decoded frames through `PcmSource` and does the math.
    #[error("AIFF association requires a container reader supplied by the caller")]
    AiffReaderUnavailable,

    #[error("[Errno 2] No such file or directory: '{0}'")]
    NotFound(PathBuf),

    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("window length is not a usable number of frames")]
    InvalidWindowLength,

    /// `_feature` divides by the length of each quarter-window segment, so a
    /// window of one, two or three samples raises `ZeroDivisionError` in the
    /// oracle. Unreachable through `extract_feature_sequence`, whose window is
    /// floored at 128 frames.
    #[error("a feature window of {0} samples has an empty envelope segment")]
    EmptyEnvelopeSegment(usize),
}
