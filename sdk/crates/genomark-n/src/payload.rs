use crate::crc24::crc24_bits;
use crate::error::GenoMarkNError;
use crate::params::{
    LOCATOR_PREFIX_BITS, MESSAGE_BITS, NAMESPACE_BITS, PAYLOAD_BITS, PAYLOAD_BYTES,
    PAYLOAD_VERSION, VERSION_BITS,
};

/// The 32 information bits GenoMark-N carries, plus the CRC-24 that gates acceptance.
///
/// The locator is the leading 25 bits of the SAME registry key GenoMark-Q's 48-bit locator
/// prefixes (spec 3.1). It is a bucket, not an identity: 25 bits over a registry of any size
/// collides by design, and a caller that resolves a bucket holding more than one candidate must
/// report `ambiguous_binding` rather than choose (spec 5.3). This type neither derives the locator
/// nor validates it against a registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payload {
    version: u8,
    namespace: u8,
    locator_prefix: u32,
}

fn check(field: &'static str, value: u64, bits: u32) -> Result<(), GenoMarkNError> {
    let max = (1u64 << bits) - 1;
    if value > max {
        return Err(GenoMarkNError::PayloadFieldRange {
            field,
            found: value,
            max,
            bits,
        });
    }
    Ok(())
}

impl Payload {
    /// IMPORTANT: the version field is checked for EQUALITY, not just for range. Spec 3.4 counts
    /// that syntactic check into the false-accept budget, so the decoder applies it; a payload
    /// carrying any other version could therefore be embedded and never returned. Rejecting it
    /// here makes that a visible error at embed rather than a silent zero in a recovery row.
    pub fn new(version: u8, namespace: u8, locator_prefix: u32) -> Result<Self, GenoMarkNError> {
        check("version", u64::from(version), VERSION_BITS)?;
        if version != PAYLOAD_VERSION {
            return Err(GenoMarkNError::PayloadVersion {
                found: version,
                expected: PAYLOAD_VERSION,
            });
        }
        check("namespace", u64::from(namespace), NAMESPACE_BITS)?;
        check(
            "locator_prefix",
            u64::from(locator_prefix),
            LOCATOR_PREFIX_BITS,
        )?;
        Ok(Self {
            version,
            namespace,
            locator_prefix,
        })
    }

    /// The leading 25 bits of a GenoMark-Q 48-bit locator, which is what N carries.
    pub fn from_q_locator(namespace: u8, q_locator: u64) -> Result<Self, GenoMarkNError> {
        check("q_locator", q_locator, 48)?;
        let prefix = (q_locator >> (48 - u64::from(LOCATOR_PREFIX_BITS))) as u32;
        Self::new(PAYLOAD_VERSION, namespace, prefix)
    }

    pub const fn version(&self) -> u8 {
        self.version
    }

    pub const fn namespace(&self) -> u8 {
        self.namespace
    }

    pub const fn locator_prefix(&self) -> u32 {
        self.locator_prefix
    }

    fn packed(self) -> u32 {
        (u32::from(self.version) << 29) | (u32::from(self.namespace) << 25) | self.locator_prefix
    }

    pub fn to_bits(self) -> [u8; PAYLOAD_BITS] {
        let packed = self.packed();
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
        let mut packed = 0u32;
        for &bit in &bits[..PAYLOAD_BITS] {
            packed = (packed << 1) | u32::from(bit & 1);
        }
        Self::new(
            ((packed >> 29) & 0x7) as u8,
            ((packed >> 25) & 0xF) as u8,
            packed & 0x01FF_FFFF,
        )
        .ok()
    }

    pub fn to_bytes(self) -> [u8; PAYLOAD_BYTES] {
        self.packed().to_be_bytes()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, GenoMarkNError> {
        let array: [u8; PAYLOAD_BYTES] =
            bytes
                .try_into()
                .map_err(|_| GenoMarkNError::PayloadLength {
                    found: bytes.len(),
                    expected: PAYLOAD_BYTES,
                })?;
        let packed = u32::from_be_bytes(array);
        Self::new(
            ((packed >> 29) & 0x7) as u8,
            ((packed >> 25) & 0xF) as u8,
            packed & 0x01FF_FFFF,
        )
    }

    /// The 56 bits the encoder embeds: 32 information bits then the CRC-24 over them, MSB first.
    pub fn to_message_bits(self) -> [u8; MESSAGE_BITS] {
        let info = self.to_bits();
        let crc = crc24_bits(&info);
        let mut message = [0u8; MESSAGE_BITS];
        message[..PAYLOAD_BITS].copy_from_slice(&info);
        for (offset, slot) in message[PAYLOAD_BITS..].iter_mut().enumerate() {
            *slot = ((crc >> (23 - offset)) & 1) as u8;
        }
        message
    }
}

/// Accepts a 56-bit hard decision only when its CRC-24 checks AND its version field is the one this
/// build writes. Spec 3.4 requires the syntactic check: the CRC alone is 2^-24 per trial, and the
/// version narrows the accepting set further at no cost.
pub fn accept_message(message: &[u8]) -> Option<Payload> {
    if message.len() != MESSAGE_BITS {
        return None;
    }
    let mut carried = 0u32;
    for &bit in &message[PAYLOAD_BITS..] {
        carried = (carried << 1) | u32::from(bit & 1);
    }
    if crc24_bits(&message[..PAYLOAD_BITS]) != carried {
        return None;
    }
    // `Payload::from_bits` is where the version equality check lives, so a decode carrying any
    // other version fails here rather than being returned as an identity.
    Payload::from_bits(&message[..PAYLOAD_BITS])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_bits_bytes_and_the_crc_gate() {
        let payload = Payload::new(PAYLOAD_VERSION, 0xB, 0x00AB_CDEF).expect("fields are in range");
        assert_eq!(Payload::from_bits(&payload.to_bits()), Some(payload));
        assert_eq!(Payload::from_bytes(&payload.to_bytes()).ok(), Some(payload));
        assert_eq!(accept_message(&payload.to_message_bits()), Some(payload));
    }

    /// The CRC is the only gate the detector has. Any single flipped bit anywhere in the 56 must
    /// fail it, or the false-accept count in spec 3.4 does not hold.
    #[test]
    fn every_single_bit_flip_is_rejected() {
        let payload = Payload::new(PAYLOAD_VERSION, 3, 0x0012_3456).expect("fields are in range");
        let message = payload.to_message_bits();
        for index in 0..MESSAGE_BITS {
            let mut flipped = message;
            flipped[index] ^= 1;
            assert!(accept_message(&flipped).is_none(), "bit {index} accepted");
        }
    }

    #[test]
    fn takes_the_leading_25_bits_of_a_q_locator() {
        let q = 0x0000_ABCD_EF12_3456u64 & 0x0000_FFFF_FFFF_FFFF;
        let payload = Payload::from_q_locator(2, q).expect("locator fits 48 bits");
        assert_eq!(payload.locator_prefix(), (q >> 23) as u32);
    }
}

#[cfg(test)]
mod version_gate {
    use super::*;

    /// The decoder refuses any version but this build's, so embedding one would produce a mark
    /// that can never be returned. That must be a visible error, not a silent zero.
    #[test]
    fn refuses_to_build_a_payload_the_decoder_could_never_return() {
        let other = if PAYLOAD_VERSION == 0 { 1 } else { 0 };
        assert!(Payload::new(other, 3, 9).is_err());
        assert!(Payload::from_bytes(&[0x47, 0x54, 0x01, 0x9A]).is_err());
        assert!(
            Payload::from_bytes(&Payload::new(PAYLOAD_VERSION, 3, 9).unwrap().to_bytes()).is_ok()
        );
    }
}
