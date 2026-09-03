//! c2pa-rs integration for the audio provenance wrapper.
//!
//! Owns the claim-v2 manifest definition, the format router that decides embedded vs
//! sidecar from the actual bytes of a file, signing through c2pa-rs, and the mapping
//! from the library's validation outcome onto [`apw_core::VerificationState`].
//!
//! IMPORTANT: nothing here establishes identity. A signer proves possession of a private
//! key; a trusted signing credential proves the key chains to an anchor the caller
//! supplied. Neither is a verified creator, owner, or rights holder.

// TODO: the CAWG cawg.identity assertion is not built here yet. When it lands it MUST wrap
// c2pa::create_signer::from_x509_identity rather than hand-rolling a second deterministic-CBOR
// and COSE_Sign1 implementation, and it MUST NOT present a self-issued chain as verified identity.

mod error;
mod format;
mod manifest;
mod signer;
mod verifier;

pub use error::{C2paError, Result};
pub use format::{
    detect_format, AssetFormat, ContainerKind, SampleFormat, AIFF_MIME, WAV_MIME,
};
pub use manifest::{
    action_for_edit_type, assertion_labels, build_manifest, digital_source_type_base,
    full_daw_provenance_unobserved, unobserved_ingredient, ExtraAssertion, Ingredient,
    IngredientRelationship, ManifestSpec, ObservedAction, UnobservedClaim,
    ACTIONS_ASSERTION_LABEL, ALGORITHMICALLY_ENHANCED, COMPOSITE_CAPTURE, DIGITAL_CAPTURE,
    FULL_DAW_PROVENANCE_CLAIM, MINOR_HUMAN_EDITS, PLUGIN_NAME, PLUGIN_VERSION, SHA256_KEY,
    UNOBSERVED_ASSERTION_LABEL,
};
pub use signer::{
    sign_asset, sign_embedded, sign_sidecar, ByteRegion, C2paSigner, HardBinding, SigningAlgorithm,
    SigningMode, SigningResult,
};
pub use verifier::{
    verify_asset, ClaimVerification, HASH_MISMATCH_CODES, TRUSTED_CODE,
};
