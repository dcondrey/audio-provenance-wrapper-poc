use crate::keystream::Schedule;
use crate::params::{BLOCK_SLOTS, PILOT_PERIOD, PREAMBLE_SLOTS};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotRole {
    Preamble(usize),
    Pilot(usize),
    Data(usize),
}

pub const fn slot_role(slot_in_block: usize) -> SlotRole {
    if slot_in_block < PREAMBLE_SLOTS {
        return SlotRole::Preamble(slot_in_block);
    }
    let body = slot_in_block - PREAMBLE_SLOTS;
    if body % PILOT_PERIOD == PILOT_PERIOD - 1 {
        SlotRole::Pilot(body / PILOT_PERIOD)
    } else {
        SlotRole::Data(body - (body + 1) / PILOT_PERIOD)
    }
}

/// Coded bit a slot carries. Preamble and pilot slots carry the keyed pattern, so the detector
/// knows them before it has decoded anything.
pub fn slot_bit(schedule: &Schedule, interleaved: &[u8], slot_in_block: usize) -> Option<u8> {
    match slot_role(slot_in_block % BLOCK_SLOTS) {
        SlotRole::Preamble(index) => schedule
            .preamble()
            .get(index)
            .map(|&polarity| u8::from(polarity > 0)),
        SlotRole::Pilot(index) => schedule
            .pilot()
            .get(index)
            .map(|&polarity| u8::from(polarity > 0)),
        SlotRole::Data(index) => interleaved.get(index).copied(),
    }
}
