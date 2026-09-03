use crate::crc32c::crc32c_bits;
use crate::error::GenoMarkError;
use crate::params::{CRC_BITS, MESSAGE_BITS, PAYLOAD_BITS, PAYLOAD_BYTES};

const VERSION_BITS: u32 = 4;
const NAMESPACE_BITS: u32 = 4;
const LOCATOR_BITS: u32 = 48;
pub const VERSION: u8 = audio_provenance_core::GENOMARK_PAYLOAD_VERSION;

/// The 56 bits GenoMark carries: 4 version, 4 namespace selector, 48 locator.
///
/// The locator is an OPAQUE registry key. Its publisher derives it with
/// `audio_provenance_core::derive_locator` from its signing key and the record's `locator_salt`, before the
/// audio is marked, and the manifest it later signs carries that salt; GenoMark neither derives it
/// nor validates it. 48 bits is an index, not an identity; collisions are expected at scale and
/// have to be resolved by re-deriving the locator from each candidate record, never assumed away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payload {
    version: u8,
    namespace: u8,
    locator: u64,
}

impl Payload {
    pub fn new(version: u8, namespace: u8, locator: u64) -> Result<Self, GenoMarkError> {
        check("version", u64::from(version), VERSION_BITS)?;
        check("namespace", u64::from(namespace), NAMESPACE_BITS)?;
        check("locator", locator, LOCATOR_BITS)?;
        Ok(Self {
            version,
            namespace,
            locator,
        })
    }

    /// The locator as `audio_provenance_core::derive_locator` produces it: six big-endian bytes.
    pub fn with_locator_bytes(
        version: u8,
        namespace: u8,
        locator: &[u8; audio_provenance_core::LOCATOR_BYTES],
    ) -> Result<Self, GenoMarkError> {
        Self::new(
            version,
            namespace,
            locator
                .iter()
                .fold(0u64, |packed, &byte| (packed << 8) | u64::from(byte)),
        )
    }

    pub const fn version(&self) -> u8 {
        self.version
    }

    pub const fn namespace(&self) -> u8 {
        self.namespace
    }

    pub const fn locator(&self) -> u64 {
        self.locator
    }

    pub fn locator_bytes(&self) -> [u8; audio_provenance_core::LOCATOR_BYTES] {
        let mut bytes = [0u8; audio_provenance_core::LOCATOR_BYTES];
        bytes.copy_from_slice(&self.locator.to_be_bytes()[2..]);
        bytes
    }

    pub fn to_bits(self) -> [u8; PAYLOAD_BITS] {
        let packed =
            (u64::from(self.version) << 52) | (u64::from(self.namespace) << 48) | self.locator;
        let mut bits = [0u8; PAYLOAD_BITS];
        for (index, slot) in bits.iter_mut().enumerate() {
            *slot = ((packed >> (PAYLOAD_BITS - 1 - index)) & 1) as u8;
        }
        bits
    }

    pub fn from_bits(bits: &[u8]) -> Option<Self> {
        if bits.len() < PAYLOAD_BITS {
            return None;
        }
        let mut packed = 0u64;
        for &bit in &bits[..PAYLOAD_BITS] {
            packed = (packed << 1) | u64::from(bit & 1);
        }
        Self::new(
            ((packed >> 52) & 0xF) as u8,
            ((packed >> 48) & 0xF) as u8,
            packed & 0x0000_FFFF_FFFF_FFFF,
        )
        .ok()
    }

    pub fn to_bytes(self) -> [u8; PAYLOAD_BYTES] {
        let bits = self.to_bits();
        let mut bytes = [0u8; PAYLOAD_BYTES];
        for (index, byte) in bytes.iter_mut().enumerate() {
            for offset in 0..8 {
                *byte |= (bits[index * 8 + offset] & 1) << (7 - offset);
            }
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, GenoMarkError> {
        if bytes.len() != PAYLOAD_BYTES {
            return Err(GenoMarkError::PayloadLength {
                found: bytes.len(),
                expected: PAYLOAD_BYTES,
            });
        }
        let mut bits = Vec::with_capacity(PAYLOAD_BITS);
        for byte in bytes {
            for shift in (0..8).rev() {
                bits.push((byte >> shift) & 1);
            }
        }
        Self::from_bits(&bits).ok_or(GenoMarkError::PayloadLength {
            found: bytes.len(),
            expected: PAYLOAD_BYTES,
        })
    }

    /// 56 payload bits followed by the 32 CRC-32C bits that gate acceptance.
    pub fn to_message(self) -> Vec<u8> {
        let bits = self.to_bits();
        let crc = crc32c_bits(&bits);
        let mut message = Vec::with_capacity(MESSAGE_BITS);
        message.extend_from_slice(&bits);
        for index in 0..CRC_BITS {
            message.push(((crc >> (CRC_BITS - 1 - index)) & 1) as u8);
        }
        message
    }

    /// The only acceptance path. A message whose CRC does not check returns `None` and the
    /// candidate is discarded silently; there is no similarity score that can override it.
    pub fn from_message(message: &[u8]) -> Option<Self> {
        if message.len() < MESSAGE_BITS {
            return None;
        }
        let expected = crc32c_bits(&message[..PAYLOAD_BITS]);
        let mut observed = 0u32;
        for &bit in &message[PAYLOAD_BITS..MESSAGE_BITS] {
            observed = (observed << 1) | u32::from(bit & 1);
        }
        if observed != expected {
            return None;
        }
        Self::from_bits(&message[..PAYLOAD_BITS])
    }
}

fn check(field: &'static str, value: u64, bits: u32) -> Result<(), GenoMarkError> {
    let max = (1u64 << bits) - 1;
    if value > max {
        return Err(GenoMarkError::PayloadFieldRange {
            field,
            found: value,
            max,
            bits,
        });
    }
    Ok(())
}
