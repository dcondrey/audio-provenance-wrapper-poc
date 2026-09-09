use crate::params::{CODED_BITS, INTERLEAVER_COLS, INTERLEAVER_ROWS};

/// Coded-bit index carried by transmitted position `position`.
///
/// Written row-wise, read column-wise, so two coded bits that were adjacent out of the encoder are
/// `INTERLEAVER_ROWS` slots apart on the wire.
pub const fn source_of(position: usize) -> usize {
    let row = position % INTERLEAVER_ROWS;
    let column = position / INTERLEAVER_ROWS;
    row * INTERLEAVER_COLS + column
}

pub fn interleave(coded: &[u8]) -> Vec<u8> {
    (0..CODED_BITS)
        .map(|position| coded.get(source_of(position)).copied().unwrap_or(0))
        .collect()
}

pub fn deinterleave(received: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0f64; CODED_BITS];
    for position in 0..CODED_BITS {
        let source = source_of(position);
        if let (Some(value), Some(slot)) = (received.get(position), out.get_mut(source)) {
            *slot = *value;
        }
    }
    out
}
