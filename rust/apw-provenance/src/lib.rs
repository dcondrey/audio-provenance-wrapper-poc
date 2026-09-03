//! The vendor-neutral provenance seam: identity, soft binding, registry and
//! verification behind one trait, with an operational local reference
//! implementation and the contract a hosted service would have to satisfy.
//!
//! Two rules this crate exists to keep enforceable:
//!
//! - A self-issued chain proves key possession, NOT verified identity. The local
//!   provider's identity proof level is `user_declared` and nothing in this crate
//!   upgrades it.
//! - A missing mark is never proof of synthetic origin. `nothing_found` records
//!   the absence of provenance data; see [`apw_core::NOTHING_FOUND_NORMATIVE_NOTE`].
//!
//! No vendor name appears in a trait, a type, a field, or anything reaching a
//! signed manifest. A vendor-named adapter implementing [`ProvenanceProvider`] is
//! the supported way to plug a hosted service in.

mod chain;
mod error;
mod hardware;
mod local;
mod mark;
mod pcm;
mod provider;
mod remote;

use std::path::Path;

pub use chain::{
    hex_decode, hex_lower, issue_leaf, issue_root, key_id_of, validate_certificate_chain,
    ChainValidation, CERT_VALIDITY_DAYS, CHAIN_VALID_REASON, DEFAULT_CREATOR_COMMON_NAME,
    ORGANIZATION_NAME, ROOT_COMMON_NAME, ROOT_VALIDITY_DAYS, SIGNING_ALGORITHM,
};
pub use error::{ProvenanceError, Result, RevokedKeyError};
pub use hardware::{
    detect_hardware_provider, DeviceIdentity, HardwareBinding, HardwareCosignature,
    HardwareProvider, SoftwareProvider, DEFAULT_DEMO_KEY_PATH, DEFAULT_DEVICE_KEY_PATH,
};
pub use local::{expand_user, LocalReferenceProvider, DEFAULT_STORE};
pub use mark::{
    descriptor_similarity, mark_limits, mark_mechanism, quantise_descriptor, DescriptorError,
    FeatureSource, PcmFeatureSource, PcmFormat, PcmReader, WindowFeature, MARK_BUCKET_TOLERANCE,
    MARK_MATCH_THRESHOLD, MARK_WINDOW_COUNT, MARK_WINDOW_SECONDS,
};
pub use provider::{
    IdentityReport, MarkAttachment, MatchedBy, ProvenanceProvider, RecoveredMark, RegistryReceipt,
    RevocationRecord, SigningIdentity, SigningMaterial, SigningRecord, VerificationOutcome,
};
pub use remote::{
    RemoteProvenanceProvider, RemoteRequirement, DEFAULT_BASE_URL, EMBED_MARK_REQUIREMENT,
    IDENTITY_REQUIREMENT, RECOVER_MARK_REQUIREMENT, REGISTER_REQUIREMENT, REMOTE_AUTH_MODEL,
    REMOTE_REQUIREMENTS, REVOKE_REQUIREMENT, SIGNING_HISTORY_REQUIREMENT,
    SIGNING_MATERIAL_REQUIREMENT, SIGN_CLAIM_REQUIREMENT, VERIFY_REQUIREMENT,
};

pub const PROVIDER_ENV_VAR: &str = "APW_PROVENANCE_PROVIDER";

/// Select a provenance provider.
///
/// The local reference provider is the default. A remote provider is used only
/// when explicitly requested, and every one of its methods returns the API
/// contract the service would have to satisfy.
pub fn detect_provider(
    store_dir: &Path,
    provider_name: Option<&str>,
) -> Result<Box<dyn ProvenanceProvider>> {
    let requested = provider_name
        .map(str::to_string)
        .or_else(|| std::env::var(PROVIDER_ENV_VAR).ok())
        .unwrap_or_else(|| "local".to_string())
        .trim()
        .to_lowercase();

    if requested == "remote" {
        log::warn!(
            "Provenance provider 'remote' requested; no remote service is implemented, so every \
             call returns its API requirement"
        );
        return Ok(Box::new(RemoteProvenanceProvider::default()));
    }
    if requested != "local" {
        log::warn!(
            "Unknown provenance provider {requested:?} requested; falling back to the local \
             reference provider"
        );
    }
    let store_dir = expand_user(store_dir);
    log::info!(
        "Using the local reference provenance provider at {}",
        store_dir.display()
    );
    Ok(Box::new(LocalReferenceProvider::new(&store_dir)?))
}
