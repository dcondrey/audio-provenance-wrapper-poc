#![cfg_attr(not(feature = "std"), no_std)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

extern crate alloc;

pub mod canonical;
pub mod error;
pub mod hashing;
pub mod locator;
pub mod signing;
pub mod vocabulary;

pub use canonical::{CANONICALIZATION_ID, MAX_DEPTH, canonical_json, parse_signing_input};
pub use error::{
    CanonicalJsonError, CodedError, CoreError, KeyError, LocatorError, SignatureError,
    VocabularyError,
};
pub use hashing::{GENESIS, sha256, sha256_hex, window_hash};
pub use locator::{
    GENOMARK_PAYLOAD_VERSION, LOCATOR_BYTES, LOCATOR_DOMAIN, LOCATOR_SALT_BYTES,
    LOCATOR_SALT_HEX_LEN, LocatorSalt, derive_locator, locator_from_signed_manifest,
};
pub use signing::{
    Canonicalization, KeyPossessionProof, ManifestSigner, PortableSignature, SignatureAlgorithm,
    SignerIdentity, SigningKey, TrustScope, signer_id_for_public_key, unsigned_manifest_view,
    verify_domain_separated, verify_manifest_signature,
};
pub use vocabulary::{
    AssociationClaim, AssociationStatus, CoverageStatus, ObservationCounters, ObservationCoverage,
    ProofLevel, VerificationStatus,
};
