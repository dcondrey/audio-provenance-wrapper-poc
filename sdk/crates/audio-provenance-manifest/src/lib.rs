//! The signed provenance record: the data model everything else reads and writes.
//!
//! # The structural invariant
//!
//! Bytes that have merely parsed are an [`UnverifiedManifest`]. A record whose Ed25519 signature
//! verified over its canonical bytes, and whose ported invariants raised no error, is a
//! [`Manifest`]. They are different types on purpose:
//!
//! - Every function that evaluates a binding, reports a claim, reads a coverage status or names a
//!   signer is a method on [`Manifest`]. None accepts [`UnverifiedManifest`].
//! - [`UnverifiedManifest::admit`] is the ONLY way to obtain a [`Manifest`]. There is no public
//!   constructor, no `Deserialize`, and no `From` conversion.
//!
//! So the guarantee is checkable by reading, not by trusting a convention: grep for
//! `UnverifiedManifest`, and its whole surface is the bytes the caller already held plus the
//! unchecked signature envelope.
//!
//! # Two record families
//!
//! [`ManifestSchema::ApwV0`] is the audio-provenance POC's `audio-provenance-manifest-v0`, whose
//! invariants are ported faithfully in [`invariants`]. [`ManifestSchema::AudioProvenanceV1`] is the
//! Audio Provenance record. A caller names the family it intends to admit; the untrusted `schema` string in
//! the document is compared against that choice and never used to make it.
//!
//! # What this crate does not do
//!
//! No crypto of its own: `audio-provenance-core` owns canonicalisation, Ed25519 and the vocabulary, and
//! this crate reuses [`audio_provenance_core::ProofLevel`], [`audio_provenance_core::AssociationClaim`] and
//! [`audio_provenance_core::ObservationCoverage`] rather than restating them. No filesystem or network I/O:
//! every entry point takes bytes. No C2PA signing, and no COSE or X.509 verification; [`c2pa`] says
//! exactly what it reads.
//!
//! It also verifies only `portable_signature`. The POC computes `manifest_signature`'s
//! `signed_content_hash` with Python's `json.dumps(..., sort_keys=True, separators=(",",":"))` and
//! no `ensure_ascii` argument, so that block is canonicalised under `apw-json-sort-ascii-v0`, which
//! `audio-provenance-core` does not implement. Checking it here with `apw-json-sort-v1` would silently
//! disagree on any manifest carrying non-ASCII.

#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

extern crate alloc;

pub mod binding;
pub mod c2pa;
pub mod draft;
pub mod error;
pub mod finding;
pub mod invariants;
pub mod manifest;
pub mod proof;
pub mod unverified;

pub use binding::{AudioFingerprint, BindingOutcome, HardBinding, MAX_FINGERPRINT_HEX_LEN};
pub use c2pa::{
    C2paHardBinding, C2paStore, Exclusion, HARD_BINDING_LABEL, MAX_STORE_BYTES, StoreLocation,
    sidecar_path_for,
};
pub use draft::{ManifestDraft, seal_unsigned_value};
pub use error::{C2paError, MAX_MANIFEST_BYTES, ManifestError};
pub use finding::{Finding, Severity, has_errors};
pub use invariants::{
    APW_SCHEMA_ID, C2PA_VALIDATION_STATES, MAX_PROOF_VALUE_DEPTH, c2pa_validation_state_to_status,
    validate_apw_invariants, validate_audio_provenance_invariants,
};
pub use manifest::{
    AUDIO_PROVENANCE_SCHEMA_ID, C2paClaimRecord, C2paClaimStatus, LOCATOR_BYTES, Manifest,
    ManifestSchema, MarkBinding, ProvenanceClaim, SignerIdentityBinding,
};
pub use proof::{Degraded, cap_proof_level, proof_rank};
pub use unverified::{AdmissionFailure, UnverifiedManifest};
