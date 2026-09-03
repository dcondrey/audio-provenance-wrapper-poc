//! The stable exit-code contract.
//!
//! CI reads these numbers, so they are a published interface and not a rendering detail.

use audio_provenance_core::VerificationStatus;

pub const VERIFIED: u8 = 0;
pub const CHANGED: u8 = 1;
pub const UNTRUSTED: u8 = 2;
pub const NOT_FOUND: u8 = 3;
/// A rung could not run: the registry was unreachable, an index would not parse.
pub const INCOMPLETE: u8 = 4;
pub const USAGE: u8 = 64;
pub const INPUT: u8 = 65;
pub const INTERNAL: u8 = 70;

/// IMPORTANT: `incomplete` overrides ONLY `not_found`. "The registry was down" is not "this file is
/// unregistered", and that is the single verdict an unfinished search misreports. A `changed` or an
/// `untrusted` rests on a positive finding the search already made, so a later rung that could not
/// run cannot have produced it and must not overwrite it.
pub const fn for_verdict(status: VerificationStatus, incomplete: bool) -> u8 {
    match status {
        VerificationStatus::Verified => VERIFIED,
        VerificationStatus::Changed => CHANGED,
        VerificationStatus::Untrusted => UNTRUSTED,
        VerificationStatus::NotFound => {
            if incomplete {
                INCOMPLETE
            } else {
                NOT_FOUND
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the whole status x incomplete table. A code that moves breaks every CI job keyed to it,
    /// and the collision worth guarding is `untrusted` = 2 against clap's own usage exit of 2.
    #[test]
    fn every_status_maps_to_its_documented_code() {
        let table = [
            (VerificationStatus::Verified, false, 0),
            (VerificationStatus::Verified, true, 0),
            (VerificationStatus::Changed, false, 1),
            (VerificationStatus::Changed, true, 1),
            (VerificationStatus::Untrusted, false, 2),
            (VerificationStatus::Untrusted, true, 2),
            (VerificationStatus::NotFound, false, 3),
            (VerificationStatus::NotFound, true, 4),
        ];
        for (status, incomplete, expected) in table {
            assert_eq!(
                for_verdict(status, incomplete),
                expected,
                "{status:?} incomplete={incomplete}"
            );
        }
        assert_ne!(USAGE, UNTRUSTED);
    }
}
