use alloc::string::String;
use sha2::{Digest, Sha256};

pub const GENESIS: &str = "genesis";

const SAMPLES_PER_CHUNK: usize = 256;

/// SHA-256 over the ASCII bytes of `prev` followed by the raw little-endian `f32` bytes of
/// `samples`. Byte-identical to `AudioObserver::computeChainedHash` in the POC's C++ and to the
/// `hashlib.sha256(previous.encode() + struct.pack("<Nf", *window))` in `synthetic_rehearsal.py`.
/// An empty `prev` means the genesis window, matching `previousHash.isEmpty()` in the C++.
pub fn window_hash(prev: &str, samples: &[f32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(if prev.is_empty() {
        GENESIS.as_bytes()
    } else {
        prev.as_bytes()
    });
    // PERF: the hasher is fed in blocks rather than four bytes at a time; audio windows run to
    // thousands of samples per call.
    let mut block = [0u8; SAMPLES_PER_CHUNK * 4];
    for chunk in samples.chunks(SAMPLES_PER_CHUNK) {
        for (slot, sample) in block.chunks_exact_mut(4).zip(chunk.iter()) {
            slot.copy_from_slice(&sample.to_le_bytes());
        }
        hasher.update(&block[..chunk.len() * 4]);
    }
    hex::encode(hasher.finalize())
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(sha256(bytes))
}
