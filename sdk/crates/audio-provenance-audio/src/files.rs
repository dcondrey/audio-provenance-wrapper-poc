use std::path::Path;

use crate::buffer::AudioBuffer;
use crate::decode::{DecodeLimits, decode_bytes};
use crate::error::AudioError;
use crate::wav::{BitDepth, encode};

/// The only filesystem entry points in the crate; everything else works on
/// buffers so the DSP paths stay usable from WASM.
pub fn decode_file(path: &Path, limits: &DecodeLimits) -> Result<AudioBuffer, AudioError> {
    let metadata = std::fs::metadata(path)?;
    let len = metadata.len();
    if len > limits.max_bytes as u64 {
        return Err(AudioError::ByteLimitExceeded {
            limit: limits.max_bytes,
            found: usize::try_from(len).unwrap_or(usize::MAX),
        });
    }
    let bytes = std::fs::read(path)?;
    decode_bytes(&bytes, limits)
}

pub fn encode_wav_file(
    path: &Path,
    buffer: &AudioBuffer,
    depth: BitDepth,
) -> Result<(), AudioError> {
    std::fs::write(path, encode(buffer, depth)?)?;
    Ok(())
}
