use crate::params::{
    CODE_RATE_DENOMINATOR, CODED_BITS, CONSTRAINT_LENGTH, GENERATORS, MESSAGE_BITS, TAIL_BITS,
    TRELLIS_STATES, TRELLIS_STEPS,
};

const SHIFT_MASK: u32 = (1u32 << CONSTRAINT_LENGTH) - 1;

fn parity(value: u32) -> u8 {
    (value.count_ones() & 1) as u8
}

fn outputs(state: usize, input: u8) -> [u8; CODE_RATE_DENOMINATOR] {
    let register = (((state as u32) << 1) | u32::from(input)) & SHIFT_MASK;
    let mut out = [0u8; CODE_RATE_DENOMINATOR];
    for (slot, generator) in out.iter_mut().zip(GENERATORS.iter()) {
        *slot = parity(register & u32::from(*generator));
    }
    out
}

const fn next_state(state: usize, input: u8) -> usize {
    ((state << 1) | input as usize) & (TRELLIS_STATES - 1)
}

/// K=9 rate-1/3 encoder, zero-tail terminated. `message` is the 88 payload+CRC bits; the 8 tail
/// bits are appended here so a caller cannot forget them and leave the trellis unterminated.
pub fn encode(message: &[u8]) -> Vec<u8> {
    let mut state = 0usize;
    let mut coded = Vec::with_capacity(CODED_BITS);
    for step in 0..TRELLIS_STEPS {
        let input = if step < MESSAGE_BITS {
            message.get(step).copied().unwrap_or(0) & 1
        } else {
            0
        };
        coded.extend_from_slice(&outputs(state, input));
        state = next_state(state, input);
    }
    coded
}

/// Soft-decision Viterbi. `llr[i]` is positive for a received one and negative for a received zero;
/// its magnitude is the reliability. Returns the 88 message bits with the tail stripped.
pub fn decode(llr: &[f64]) -> Option<Vec<u8>> {
    if llr.len() < CODED_BITS {
        return None;
    }
    let mut metric = vec![f64::NEG_INFINITY; TRELLIS_STATES];
    let mut next = vec![f64::NEG_INFINITY; TRELLIS_STATES];
    metric[0] = 0.0;
    let mut survivors = vec![0u8; TRELLIS_STEPS * TRELLIS_STATES];

    for step in 0..TRELLIS_STEPS {
        let received = &llr[step * CODE_RATE_DENOMINATOR..(step + 1) * CODE_RATE_DENOMINATOR];
        next.fill(f64::NEG_INFINITY);
        // Beyond the message the encoder is forced to zero, so only the input-0 branch exists.
        let inputs: &[u8] = if step < MESSAGE_BITS { &[0, 1] } else { &[0] };
        for (state, &from) in metric.iter().enumerate() {
            if from == f64::NEG_INFINITY {
                continue;
            }
            for &input in inputs {
                let symbols = outputs(state, input);
                let mut branch = from;
                for (index, &symbol) in symbols.iter().enumerate() {
                    branch += if symbol == 1 {
                        received[index]
                    } else {
                        -received[index]
                    };
                }
                let target = next_state(state, input);
                if branch > next[target] {
                    next[target] = branch;
                    // The input bit is recoverable from the target state's least significant bit,
                    // so the survivor only has to remember where the branch came from.
                    survivors[step * TRELLIS_STATES + target] = state as u8;
                }
            }
        }
        core::mem::swap(&mut metric, &mut next);
    }

    if metric[0] == f64::NEG_INFINITY {
        return None;
    }
    let mut state = 0usize;
    let mut bits = vec![0u8; TRELLIS_STEPS];
    for step in (0..TRELLIS_STEPS).rev() {
        bits[step] = (state & 1) as u8;
        state = usize::from(survivors[step * TRELLIS_STATES + state]);
    }
    bits.truncate(TRELLIS_STEPS - TAIL_BITS);
    Some(bits)
}
