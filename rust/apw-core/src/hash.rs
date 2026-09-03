use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{CoreError, Result};

pub const HASH_CHUNK_BYTES: usize = 1 << 20;

/// Absurdity bound on an untrusted manifest's `byte_length`. It never sizes an
/// allocation (hashing is chunked), but a bound keeps a crafted value from
/// turning verification into an unbounded read of a growing file.
pub const MAX_PREFIX_BYTES: u64 = 1 << 40;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(bytes);
    hex_lower(&digest.finalize())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    Ok(sha256_file_with_size(path)?.0)
}

pub fn sha256_file_with_size(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path).map_err(|source| CoreError::io(path, source))?;
    sha256_reader(&mut file).map_err(|error| relabel_io(error, path))
}

/// Hashes exactly `byte_length` bytes without loading the file.
/// [`CoreError::ShortPrefix`] if the file ends first.
pub fn sha256_prefix(path: &Path, byte_length: u64) -> Result<String> {
    if byte_length > MAX_PREFIX_BYTES {
        return Err(CoreError::PrefixTooLong {
            got: byte_length,
            max: MAX_PREFIX_BYTES,
        });
    }
    let mut file = File::open(path).map_err(|source| CoreError::io(path, source))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    let mut remaining = byte_length;
    while remaining > 0 {
        let want = usize::try_from(remaining).unwrap_or(HASH_CHUNK_BYTES).min(HASH_CHUNK_BYTES);
        let slice = buffer
            .get_mut(..want)
            .ok_or(CoreError::PrefixTooLong {
                got: byte_length,
                max: MAX_PREFIX_BYTES,
            })?;
        let read = file
            .read(slice)
            .map_err(|source| CoreError::io(path, source))?;
        if read == 0 {
            return Err(CoreError::ShortPrefix {
                path: path.to_path_buf(),
                expected: byte_length,
            });
        }
        let hashed = slice.get(..read).ok_or(CoreError::ShortPrefix {
            path: path.to_path_buf(),
            expected: byte_length,
        })?;
        digest.update(hashed);
        remaining -= read as u64;
    }
    Ok(hex_lower(&digest.finalize()))
}

pub fn sha256_reader<R: Read>(reader: &mut R) -> Result<(String, u64)> {
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|source| CoreError::io("<reader>", source))?;
        if read == 0 {
            break;
        }
        let chunk = buffer
            .get(..read)
            .ok_or(CoreError::KeyMaterial("short read exceeded the buffer"))?;
        digest.update(chunk);
        total = total.saturating_add(read as u64);
    }
    Ok((hex_lower(&digest.finalize()), total))
}

fn relabel_io(error: CoreError, path: &Path) -> CoreError {
    match error {
        CoreError::Io { source, .. } => CoreError::io(path, source),
        other => other,
    }
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 == 1 {
        return None;
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for index in (0..bytes.len()).step_by(2) {
        let high = bytes.get(index).copied()?;
        let low = bytes.get(index + 1).copied()?;
        let high = (high as char).to_digit(16)?;
        let low = (low as char).to_digit(16)?;
        out.push(u8::try_from(high * 16 + low).ok()?);
    }
    Some(out)
}
