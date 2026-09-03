//! The identity layer: Ed25519 trust anchors, signer records that bind a key to a name, revocation,
//! and the chain evaluation that decides whether a proven key resolves to an identity.
//!
//! ```text
//! anchor (self-signed)  --issues-->  [issuer record]*  --issues-->  leaf record  ==  a name
//! ```
//!
//! # What this crate may and may not decide
//!
//! `audio-provenance-core` fixes a self-generated signature at `signer_identity_proof_level =
//! unknown_unobserved`, and that guard is not weakened here. What this crate adds is the only
//! legitimate way past it: a chain from the signing key to an anchor the verifier configured. A
//! chain that reaches a trusted anchor is `externally_verified`. Everything else is
//! `unknown_unobserved`. There is no middle level and no caller-settable one: `genotrace` derives
//! the level from its own status mapping, and [`AnchoredTrustStore`] can only report reached or
//! not reached.
//!
//! A self-signed anchor an operator simply chose to trust still yields `externally_verified`
//! relative to THAT anchor, which is why every vouched result carries the anchor's id and key id.
//! Trust is not a boolean; the consumer is given the anchor so they can judge it.
//!
//! # Native records, not X.509
//!
//! `docs/DESIGN_ARCHITECTURE.md` specifies X.509 for the Node crypto package. This crate diverges
//! on FORMAT and holds the same POLICY: an identity proof level is read from the anchor and never
//! upgraded locally. The reason is parsing surface. Chain validation runs on wholly untrusted
//! input, and X.509 brings DER/ASN.1, name constraints, EKU, AKI/SKI matching and a decade of
//! parser CVEs for interoperability with public CAs that no part of v1 consumes. A native record is
//! a flat JSON object read through one bounded reader, signed with the Ed25519 and
//! `apw-json-sort-v1` canonicalization the rest of the system already audits. The whole hostile
//! boundary is [`mod@reader`], which is under 200 lines.

#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod adapter;
pub mod chain;
pub mod document;
pub mod error;
mod reader;
pub mod store;
pub mod time;

pub use adapter::AnchoredTrustStore;
pub use chain::{Refusal, TrustEvaluation, VouchedIdentity};
pub use document::{
    ALGORITHM, ANCHOR_TYPE, Anchor, CHAIN_DEPTH_CEILING, Capability, RECORD_TYPE, REVOCATIONS_TYPE,
    RevocationEntry, RevocationList, RevocationReason, SignedAnchor, SignedRecord,
    SignedRevocationList, SignerRecord,
};
pub use error::TrustError;
pub use store::{STORE_FILE_NAMES, STORE_FORMAT, TrustStore};
pub use time::{Instant, Window};
