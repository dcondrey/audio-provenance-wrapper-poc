/// CRC-32C (Castagnoli), reflected form of polynomial 0x1EDC6F41, init and final xor 0xFFFFFFFF.
const POLY: u32 = 0x82F6_3B78;

fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ POLY
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

pub fn crc32c(bytes: &[u8]) -> u32 {
    let table = table();
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        let slot = ((crc ^ u32::from(byte)) & 0xFF) as usize;
        crc = (crc >> 8) ^ table[slot];
    }
    !crc
}

/// CRC over a bit sequence whose length need not be a multiple of eight. The 56-bit payload is a
/// whole number of bytes, but the accessor is bit-addressed so a future payload width does not
/// silently change what is covered.
pub fn crc32c_bits(bits: &[u8]) -> u32 {
    let mut bytes = Vec::with_capacity(bits.len().div_ceil(8));
    for chunk in bits.chunks(8) {
        let mut byte = 0u8;
        for (index, &bit) in chunk.iter().enumerate() {
            byte |= (bit & 1) << (7 - index);
        }
        bytes.push(byte);
    }
    crc32c(&bytes)
}
