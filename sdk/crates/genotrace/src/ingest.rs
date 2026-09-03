//! Stage 0: bounded, untrusted ingest.
//!
//! Bytes in, budgets checked before any work, two exact digests out. Everything here throws a
//! caller error or succeeds; nothing here can produce a provenance status.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use audio_provenance_audio::{AudioBuffer, DecodeLimits, decode_bytes};
use sha2::{Digest, Sha256};

use crate::error::GenoTraceError;

/// The read chunk, mirroring the POC's `sha256_file_with_size`.
const CHUNK_BYTES: usize = 1024 * 1024;

pub const DEFAULT_MAX_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_MAX_DURATION_SECONDS: f64 = 3600.0;

/// The canonical PCM serialization the decoded-audio digest covers.
///
/// Eight magic bytes, then channel count and sample rate as little-endian `u32`: sixteen bytes,
/// then interleaved little-endian `f32`. The magic is versioned so a later layout cannot be
/// mistaken for this one and silently produce a mismatched binding.
pub const DECODED_PCM_MAGIC: [u8; 8] = *b"GTOPCM01";

/// Budgets applied before any decode work.
///
/// There is deliberately no wall-clock decode timeout. A synchronous decoder cannot be interrupted
/// without abandoning a thread, and a timeout that only fires after the work is done bounds
/// nothing. The sample budget derived from [`Self::max_duration_seconds`] bounds the same work
/// deterministically, and a deterministic bound is the stronger guarantee.
#[derive(Debug, Clone, Copy)]
pub struct IngestLimits {
    pub max_bytes: u64,
    pub max_duration_seconds: f64,
}

impl Default for IngestLimits {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_MAX_BYTES,
            max_duration_seconds: DEFAULT_MAX_DURATION_SECONDS,
        }
    }
}

impl IngestLimits {
    fn validate(self) -> Result<(), GenoTraceError> {
        if self.max_bytes == 0 {
            return Err(GenoTraceError::InvalidOption {
                option: "max_bytes",
                reason: "must be positive".to_string(),
            });
        }
        if !self.max_duration_seconds.is_finite() || self.max_duration_seconds <= 0.0 {
            return Err(GenoTraceError::InvalidOption {
                option: "max_duration_seconds",
                reason: "must be a positive number of seconds".to_string(),
            });
        }
        Ok(())
    }

    /// Interleaved sample ceiling implied by the duration budget, at the widest sample rate and
    /// channel count the audio crate accepts. Bounded so a hostile header cannot make the product
    /// wrap.
    fn max_samples(self) -> usize {
        let widest = f64::from(audio_provenance_audio::MAX_SAMPLE_RATE)
            * (audio_provenance_audio::MAX_CHANNELS as f64)
            * self.max_duration_seconds;
        if widest >= DecodeLimits::DEFAULT_MAX_SAMPLES as f64 {
            DecodeLimits::DEFAULT_MAX_SAMPLES
        } else {
            widest as usize
        }
    }
}

/// Containers GenoTrace recognises by magic, never by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Container {
    Wav,
    Aiff,
    Mp3,
    Flac,
    Ogg,
    Mp4,
}

impl Container {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Aiff => "aiff",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::Mp4 => "mp4",
        }
    }
}

/// Sniffs the container from leading bytes.
///
/// An MP3 may open with an ID3v2 tag whose 28-bit synchsafe size says where the audio starts, or
/// straight into a frame sync. Both are accepted; a bare `ID3` header with no reachable sync is
/// not, because that is a tag with no audio behind it.
pub fn sniff(bytes: &[u8]) -> Option<Container> {
    if bytes.len() < 12 {
        return None;
    }
    if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        return Some(Container::Wav);
    }
    if bytes.starts_with(b"FORM") && matches!(bytes.get(8..12), Some(b"AIFF") | Some(b"AIFC")) {
        return Some(Container::Aiff);
    }
    if bytes.starts_with(b"fLaC") {
        return Some(Container::Flac);
    }
    if bytes.starts_with(b"OggS") {
        return Some(Container::Ogg);
    }
    if bytes.get(4..8) == Some(b"ftyp") {
        return Some(Container::Mp4);
    }
    if mpeg_sync_offset(bytes).is_some() {
        return Some(Container::Mp3);
    }
    None
}

fn mpeg_sync_offset(bytes: &[u8]) -> Option<usize> {
    let start = if bytes.starts_with(b"ID3") {
        let size = bytes.get(6..10)?;
        // Synchsafe: seven significant bits per byte, high bit always clear.
        if size.iter().any(|byte| byte & 0x80 != 0) {
            return None;
        }
        let tag_size = size
            .iter()
            .fold(0u32, |acc, byte| (acc << 7) | u32::from(*byte));
        10usize.checked_add(tag_size as usize)?
    } else {
        0
    };
    let window = bytes.get(start..)?;
    let limit = window.len().min(8192);
    window
        .get(..limit)?
        .windows(2)
        .position(|pair| pair[0] == 0xFF && (pair[1] & 0xE0) == 0xE0 && (pair[1] & 0x18) != 0x08)
}

/// A file read, hashed, sniffed and decoded, with both exact digests available.
#[derive(Debug)]
pub struct Ingested {
    bytes: Vec<u8>,
    content_sha256: String,
    content_bytes: u64,
    container: Container,
    audio: AudioBuffer,
}

impl Ingested {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    pub const fn content_bytes(&self) -> u64 {
        self.content_bytes
    }

    pub const fn container(&self) -> Container {
        self.container
    }

    pub const fn audio(&self) -> &AudioBuffer {
        &self.audio
    }

    /// The second exact binding: bit-exact over samples, indifferent to container metadata.
    pub fn decoded_audio_sha256(&self) -> String {
        decoded_audio_sha256(&self.audio)
    }
}

/// SHA-256 over the canonical PCM serialization described at [`DECODED_PCM_MAGIC`].
pub fn decoded_audio_sha256(audio: &AudioBuffer) -> String {
    let mut hasher = Sha256::new();
    hasher.update(DECODED_PCM_MAGIC);
    hasher.update((audio.channels() as u32).to_le_bytes());
    hasher.update(audio.sample_rate().to_le_bytes());
    // PERF: a four-byte update per sample costs one call per sample on a buffer that can hold a
    // hundred million of them. Batching into a chunk hashes the identical byte string.
    let mut chunk = Vec::with_capacity(CHUNK_BYTES);
    for sample in audio.to_interleaved() {
        chunk.extend_from_slice(&sample.to_le_bytes());
        if chunk.len() >= CHUNK_BYTES {
            hasher.update(&chunk);
            chunk.clear();
        }
    }
    hasher.update(&chunk);
    hex::encode(hasher.finalize())
}

/// Streaming SHA-256 with the byte count kept, because the length is part of the binding.
///
/// Exposed for a caller that needs only the digest of a file too large to hold. GenoTrace's own
/// ingest keeps the bytes resident regardless: the decoder and the container scan both need the
/// whole buffer, so streaming there would save nothing and cost a second read.
pub fn hash_reader<R: Read>(reader: &mut R, limit: u64) -> Result<(String, u64), std::io::Error> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > limit {
            return Ok((String::new(), total));
        }
        hasher.update(&buffer[..read]);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

pub fn ingest_path(path: &Path, limits: IngestLimits) -> Result<Ingested, GenoTraceError> {
    limits.validate()?;
    let metadata = std::fs::metadata(path).map_err(|source| GenoTraceError::Unreadable {
        path: path.display().to_string(),
        source,
    })?;
    if metadata.len() > limits.max_bytes {
        return Err(GenoTraceError::InputTooLarge {
            found: metadata.len(),
            limit: limits.max_bytes,
        });
    }

    let mut file = File::open(path).map_err(|source| GenoTraceError::Unreadable {
        path: path.display().to_string(),
        source,
    })?;
    let mut bytes = Vec::with_capacity(metadata.len().min(limits.max_bytes) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|source| GenoTraceError::Unreadable {
            path: path.display().to_string(),
            source,
        })?;
    ingest_bytes(bytes, limits)
}

pub fn ingest_bytes(bytes: Vec<u8>, limits: IngestLimits) -> Result<Ingested, GenoTraceError> {
    limits.validate()?;
    let content_bytes = bytes.len() as u64;
    if content_bytes > limits.max_bytes {
        return Err(GenoTraceError::InputTooLarge {
            found: content_bytes,
            limit: limits.max_bytes,
        });
    }
    let container = sniff(&bytes).ok_or(GenoTraceError::UnrecognisedContainer)?;

    let mut cursor = std::io::Cursor::new(&bytes);
    let (content_sha256, hashed) =
        hash_reader(&mut cursor, limits.max_bytes).map_err(|source| {
            GenoTraceError::Unreadable {
                path: "<memory>".to_string(),
                source,
            }
        })?;
    if hashed != content_bytes || content_sha256.is_empty() {
        return Err(GenoTraceError::InputTooLarge {
            found: hashed,
            limit: limits.max_bytes,
        });
    }

    let decode_limits = DecodeLimits::new(
        usize::try_from(limits.max_bytes).unwrap_or(usize::MAX),
        limits.max_samples(),
    );
    let audio =
        decode_bytes(&bytes, &decode_limits).map_err(|error| GenoTraceError::Undecodable {
            reason: error.to_string(),
        })?;
    let duration = audio.duration_seconds();
    if duration > limits.max_duration_seconds {
        return Err(GenoTraceError::DurationTooLong {
            found: duration,
            limit: limits.max_duration_seconds,
        });
    }

    Ok(Ingested {
        bytes,
        content_sha256,
        content_bytes,
        container,
        audio,
    })
}
