/// CRC-24 as OpenPGP specifies it (RFC 4880 section 6.1): polynomial 0x864CFB, initial value
/// 0xB704CE, MSB-first, no reflection and no final xor. Spec 3.1 fixes this and spec 3.4 counts
/// its width into the false-accept budget, so it is not interchangeable with another CRC-24.
const POLY: u32 = 0x0086_4CFB;
const INIT: u32 = 0x00B7_04CE;
const MASK: u32 = 0x00FF_FFFF;

pub fn crc24_bits(bits: &[u8]) -> u32 {
    let mut register = INIT;
    for &bit in bits {
        register ^= u32::from(bit & 1) << 23;
        register <<= 1;
        if register & 0x0100_0000 != 0 {
            register ^= POLY;
        }
    }
    register & MASK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_of(bytes: &[u8]) -> Vec<u8> {
        bytes
            .iter()
            .flat_map(|byte| (0..8).rev().map(move |shift| (byte >> shift) & 1))
            .collect()
    }

    /// RFC 4880's own worked value for the empty message, plus the ASCII vector every
    /// independent implementation of this CRC agrees on.
    #[test]
    fn matches_rfc4880_vectors() {
        assert_eq!(crc24_bits(&[]), 0x00B7_04CE);
        assert_eq!(crc24_bits(&bits_of(b"123456789")), 0x0021_CF02);
    }

    #[test]
    fn detects_every_single_bit_error_in_a_32_bit_message() {
        let message = bits_of(&[0x9E, 0x3B, 0x00, 0x41]);
        let reference = crc24_bits(&message);
        for index in 0..message.len() {
            let mut flipped = message.clone();
            flipped[index] ^= 1;
            assert_ne!(crc24_bits(&flipped), reference, "bit {index}");
        }
    }
}
