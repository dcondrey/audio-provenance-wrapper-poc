//! An adversarial campaign against `genomark-lepqim-v1`.
//!
//! # Why this crate exists separately
//!
//! `audio-provenance-bench` owns the attacks and knows nothing about any particular watermark;
//! `genomark` depends on `audio-provenance-bench`, so the wiring that points one at the other cannot live
//! in either. This crate is that wiring and nothing else. It reads only PUBLISHED items out of
//! `genomark`: the band geometry, the lattice step, the convolutional code, the interleaver and the
//! slot roles. It never reads a profile key it was not given, and the attacker instances it builds
//! are keyed differently from the victim.
//!
//! # The premise every row rests on
//!
//! The victim's key is secret. The geometry is not. Everything in [`campaign`] that is labelled
//! `spec_only` or `one_marked_file` is reachable by someone who has read `GENOMARK_SPEC.md` and
//! holds no key at all.

pub mod campaign;
pub mod spec;
