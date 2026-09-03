//! C2PA interop: claim structure, assertion-store integrity, and the status mapping.
//!
//! # Scope, stated plainly
//!
//! This crate READS. It parses the C2PA claim out of a manifest store, recomputes every hashed URI
//! the claim commits to, and maps a validator's findings onto Audio Provenance's four verification
//! statuses. It locates no store and recomputes no hard binding itself: both already live in
//! [`audio_provenance_manifest::C2paStore`], and the whole point of an interop crate is to exercise the
//! code that ships rather than a second copy of it.
//!
//! # NOT implemented, and not claimed
//!
//! - **No signing.** Nothing here writes a manifest, a claim, or a signature.
//! - **No COSE_Sign1 verification.** The `c2pa.signature` box is located and reported as present;
//!   its contents are never parsed or verified.
//! - **No X.509 chain building, no trust list, no revocation, no timestamp check.**
//!
//! Because of the middle two, this crate can never conclude a C2PA validation state on its own.
//! [`classify`] takes the signature and credential facts as INPUTS, and the value it can honestly
//! supply for the first, [`ClaimSignatureEvidence::NotEvaluated`], classifies as
//! `mark_found_claim_not_trusted`. A matching hard binding with intact assertion hashes proves the
//! bytes have not moved since signing. It does not prove anyone trustworthy signed them.
//!
//! # What was checked against a real tool
//!
//! `tests/conformance.rs` pins the claim fields, the hashed-URI digests, the hard-binding
//! exclusions and the recompute verdict as literals taken from `c2patool 0.26.68` and from the
//! POC's own signer output. `tests/c2patool_differential.rs` re-derives them by running c2patool
//! and is `#[ignore]`d, because it needs a binary that is not present on every machine.
//!
//! Checked on four containers: an embedded WAV, an AIFF with a detached sidecar, a WAV carrying two
//! manifests, and a store with an undeclared assertion spliced in. c2patool and this crate agree on
//! the active manifest, the claim structure, every hashed URI, the hard-binding verdict, and the
//! refusal of the tampered store.
//!
//! Still unexercised by any real artifact, and so NOT claimed: claim v1 (`c2pa.claim`), a hash
//! algorithm other than SHA-256, `c2pa.hash.boxes` and any BMFF binding, per-URI `alg` overrides,
//! and a store whose exclusion list has more than one range.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

extern crate alloc;

pub mod claim;
pub mod error;
pub mod jumbf;
pub mod validation;

pub use claim::{
    ASSERTION_STORE_LABEL, AssertionIntegrity, AssertionKind, C2paClaim, CLAIM_V1_LABEL,
    CLAIM_V2_LABEL, ClaimVersion, HashedUri, HashedUriCheck, HashedUriOutcome, SIGNATURE_LABEL,
    STORE_LABEL, active_manifest,
};
pub use error::ClaimError;
pub use jumbf::{LabelledBox, MAX_BOXES, MAX_DEPTH};
pub use validation::{
    C2paEvidence, C2paValidationState, ClaimSignatureEvidence, CredentialEvidence,
    HardBindingEvidence, classify,
};
