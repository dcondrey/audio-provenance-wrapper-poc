//! Frozen domain vocabulary and the two byte-exact canonicalizers for the audio
//! provenance wrapper.
//!
//! IMPORTANT: this crate MUST NOT depend on the `c2pa` crate. It is the shared
//! vocabulary every other crate in the workspace is written against, and the
//! canonicalizers here must stay byte-identical to `daemon/common.py` or a
//! signature produced by one implementation fails verification in the other.

mod canonical;
mod error;
mod evidence;
mod finding;
mod guard;
mod hash;
mod manifest;
mod ots;
mod proof;
mod pyvalue;
mod schema;
mod signature;
mod state;
mod time_anchor;
mod timestamp;

pub use canonical::{
    canonical_json, canonical_json_ascii, canonical_json_of, canonical_json_utf8,
    pretty_json_bytes, pretty_json_sorted_bytes, python_repr_f64, without_top_level_keys, Canonicalization,
    CANONICALIZATION_LOCAL, CANONICALIZATION_PORTABLE, LOCAL_SIGNATURE_EXCLUDED_KEYS,
    MAX_PROOF_VALUE_DEPTH, PORTABLE_SIGNATURE_EXCLUDED_KEYS,
};
pub use error::{CoreError, Result};
pub use evidence::{
    append_jsonl, rotated_evidence_paths, DEFAULT_EVIDENCE_BACKUPS, DEFAULT_EVIDENCE_MAX_BYTES,
};
pub use finding::{
    Finding, Severity, VerificationReport, CHANGED_CODES, QUALIFIED_SCOPE, REQUIRED_CHECKS,
};
pub use hash::{
    sha256_file, sha256_file_with_size, sha256_hex, sha256_prefix, sha256_reader,
    HASH_CHUNK_BYTES, MAX_PREFIX_BYTES,
};
pub use manifest::{
    c2pa_action_type, unavailable_c2pa_claim, AssociationStatus, C2paClaimStatus, CoverageStatus,
    ExportEvidence, IngredientEvidence, Manifest, ManifestBuilder, ReceiptStatus, StemEvidence,
    APW_VERSION, C2PA_CLAIM_SCOPE, CORE_PRINCIPLE, DEFAULT_UNOBSERVED, MANIFEST_SCHEMA,
};
pub use proof::{ProofLevel, PROOF_LEVEL_KEY};
pub use ots::{
    attestation_summary as ots_attestation_summary, check_bitcoin_attestations, double_sha256,
    evaluate_record as evaluate_ots_record, file_hash_op_name, hex as ots_hex, parse_detached,
    parse_timestamp as parse_ots_timestamp, pending_record as ots_pending_record,
    unavailable_record as ots_unavailable_record, Attestation, AttestationKind, BitcoinCheck,
    BlockHeader, CheckStatus, DetachedProof, HeaderSource, LocalHeaderSource, Op, OtsError,
    OtsFinding, Timestamp as OtsTimestamp, CALENDAR_TIMEOUT_SECONDS, DEFAULT_CALENDARS,
    MAX_CALENDAR_RESPONSE_BYTES, OP_SHA256,
};
pub use pyvalue::{is_truthy, path_name, python_eq, python_int, python_repr, python_str};
pub use schema::{validate_manifest_invariants, REQUIRED_MANIFEST_KEYS};
pub use signature::{
    signer_id_of, verify_pinned_signature, verify_portable_signature, Ed25519Signer, PortableSignature, SignatureRejection,
    PORTABLE_SIGNATURE_NOTES, PORTABLE_SIGNATURE_VALID_MESSAGE, TRUST_SCOPE_HARDWARE_PROVIDER,
    TRUST_SCOPE_LOCAL_SOFTWARE, TRUST_SCOPE_SELF_GENERATED,
};
pub use state::{LocalOutcome, VerificationState, NOTHING_FOUND_NORMATIVE_NOTE};
pub use time_anchor::{
    anchored_record, check_time_anchor, encode_timestamp_request, iso_from_ms,
    parse_timestamp_request, parse_timestamp_response, python_from_hex, unavailable_anchor_record,
    verify_time_proof, ParsedTimestampResponse, TimeAnchorError, TimeProof, DEFAULT_TSA_URL,
    MAX_TSA_RESPONSE_BYTES, TSA_TIMEOUT_SECONDS,
};
pub use timestamp::{utc_timestamp, utc_timestamp_seconds};
