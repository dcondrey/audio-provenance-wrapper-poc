//! Adversarial attacks on a spectral QIM watermark, and the cost of each one.
//!
//! # This is a different threat model from the channel matrix
//!
//! [`crate::channel`] measures INCIDENTAL degradation: what a codec, a resampler or a limiter does
//! to a mark on its way through a normal distribution path. Nothing in it is trying to remove the
//! mark. This module is the other question, and the two numbers are not interchangeable. A mark
//! that survives every codec can still be erased by an attacker who has read the specification.
//!
//! # The primitive
//!
//! Almost every removal here is one operation: measure the slot statistic the mark is quantised in,
//! decide what it should become, and drive it there. [`statistic::drive`] iterates to a MEASURED
//! target the way an embedder does, because a single spectral multiply returns only about half the
//! intended change to the next analysis of the same slot; an attack that applied one pass would
//! land at half strength and would flatter the mark. Every row reports the shift it achieved.
//!
//! # An attack that destroys the audio is not an attack
//!
//! [`report::AttackRow`] carries the perceptual cost of every removal, measured by
//! [`crate::perceptual`] against the marked file the attack started from, under exactly the
//! assumptions [`crate::perceptual::PERCEPTUAL_LIMITS`] states. A row that removes the mark at a
//! cost no listener would accept is vandalism and is reported as such.
//!
//! # Every row states what the attacker must hold
//!
//! [`report::AttackerKnowledge`] is not decoration. The difference between "needs the published
//! spec" and "needs a hundred licensee copies of one master" is the whole finding.

pub mod collusion;
pub mod denoise;
pub mod desync;
pub mod erase;
pub mod geometry;
pub mod report;
pub mod statistic;

pub use erase::{ConstantOffset, CosetTransplant, Flatten, RandomOffset, ResidueEstimate};
pub use geometry::{PairBandGeometry, PunctureRule};
pub use report::{
    AttackFamily, AttackReport, AttackRow, AttackerKnowledge, RemovalVerdict, THREAT_MODEL,
};
pub use statistic::{DEFAULT_MAX_PASSES, DriveReport, SlotTargets, analyse, drive};
