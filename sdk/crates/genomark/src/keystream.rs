use chacha20::ChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use hkdf::Hkdf;
use sha2::Sha256;

use crate::error::GenoMarkError;
use crate::params::{
    ALGORITHM_ID, BLOCK_SLOTS, DELTA, PILOT_SLOTS, PREAMBLE_SLOTS, PUBLIC_PROFILE_KEY,
};

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const DITHER_WORD: usize = 4;
const STREAM_BYTES: usize = BLOCK_SLOTS * DITHER_WORD + PREAMBLE_SLOTS + PILOT_SLOTS;

/// The dither lattice, the preamble PN and the pilot polarities for one key epoch.
///
/// The same schedule serves every block in the epoch, which is what lets a blind detector know
/// `u[s]` for a candidate block start without knowing where the file began.
#[derive(Debug, Clone)]
pub struct Schedule {
    dither: Vec<f64>,
    preamble: Vec<i8>,
    pilot: Vec<i8>,
}

impl Schedule {
    pub fn derive(profile_key: &[u8], namespace: u8, epoch: u64) -> Result<Self, GenoMarkError> {
        if profile_key.is_empty() {
            return Err(GenoMarkError::EmptyProfileKey);
        }
        let mut info = Vec::with_capacity(ALGORITHM_ID.len() + 1 + 8);
        info.extend_from_slice(ALGORITHM_ID.as_bytes());
        info.push(namespace);
        info.extend_from_slice(&epoch.to_be_bytes());

        let mut seed = [0u8; KEY_BYTES + NONCE_BYTES];
        Hkdf::<Sha256>::new(None, profile_key)
            .expand(&info, &mut seed)
            .map_err(|_| GenoMarkError::KeyDerivation { bytes: seed.len() })?;

        let mut key = [0u8; KEY_BYTES];
        let mut nonce = [0u8; NONCE_BYTES];
        key.copy_from_slice(&seed[..KEY_BYTES]);
        nonce.copy_from_slice(&seed[KEY_BYTES..]);
        let mut stream = vec![0u8; STREAM_BYTES];
        ChaCha20::new(&key.into(), &nonce.into()).apply_keystream(&mut stream);

        let mut dither = Vec::with_capacity(BLOCK_SLOTS);
        for slot in 0..BLOCK_SLOTS {
            let base = slot * DITHER_WORD;
            let word = u32::from_be_bytes([
                stream[base],
                stream[base + 1],
                stream[base + 2],
                stream[base + 3],
            ]);
            dither.push(DELTA * f64::from(word) / 4_294_967_296.0);
        }
        let mut cursor = BLOCK_SLOTS * DITHER_WORD;
        let preamble = stream[cursor..cursor + PREAMBLE_SLOTS]
            .iter()
            .map(|byte| if byte & 1 == 1 { 1i8 } else { -1i8 })
            .collect();
        cursor += PREAMBLE_SLOTS;
        let pilot = stream[cursor..cursor + PILOT_SLOTS]
            .iter()
            .map(|byte| if byte & 1 == 1 { 1i8 } else { -1i8 })
            .collect();

        Ok(Self {
            dither,
            preamble,
            pilot,
        })
    }

    pub fn public(namespace: u8, epoch: u64) -> Result<Self, GenoMarkError> {
        Self::derive(PUBLIC_PROFILE_KEY, namespace, epoch)
    }

    pub fn dither(&self, slot_in_block: usize) -> f64 {
        self.dither[slot_in_block % BLOCK_SLOTS]
    }

    pub fn preamble(&self) -> &[i8] {
        &self.preamble
    }

    pub fn pilot(&self) -> &[i8] {
        &self.pilot
    }
}
