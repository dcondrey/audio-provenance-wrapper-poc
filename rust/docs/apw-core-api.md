# apw-core frozen public API (phase 1 output)

```rust
// ============================== error ==============================
pub type Result<T> = core::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("cannot read {path}: {source}")]
    Io { path: std::path::PathBuf, #[source] source: std::io::Error },
    #[error("{path} is shorter than the bound {expected}-byte prefix")]
    ShortPrefix { path: std::path::PathBuf, expected: u64 },
    #[error("prefix byte_length {got} exceeds the {max}-byte bound")]
    PrefixTooLong { got: u64, max: u64 },
    #[error("canonical JSON rejects the non-finite number at {pointer}")]
    NonFiniteNumber { pointer: String },
    #[error("manifest nesting exceeds {max} levels at {pointer}")]
    NestingTooDeep { pointer: String, max: usize },
    #[error("invalid proof level: {value}")]
    InvalidProofLevel { value: String },
    #[error("invalid {field}: {value}")]
    InvalidEnumValue { field: &'static str, value: String },
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("Ed25519 key material is invalid: {0}")]
    KeyMaterial(&'static str),
}

// ============================== proof levels ==============================
/// IMPORTANT: every claim-bearing object in a manifest carries exactly one of
/// these. There is no "absent" variant; omission is `UnknownUnobserved`.
/// No `Ord` derive: the only ordering is `rank()`, so its "not a trust ranking"
/// caveat cannot be bypassed by an unattached `max()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofLevel {
    UnknownUnobserved,
    UserDeclared,
    Inferred,
    DirectlyObserved,
    ExternallyVerified,
}

impl ProofLevel {
    pub const ALL: [ProofLevel; 5];
    pub fn as_str(self) -> &'static str;
    pub fn parse(value: &str) -> Result<ProofLevel>;
    /// Evidence-strength order for network cap comparisons only; NOT a trust ranking.
    pub fn rank(self) -> u8;
    pub fn capped_at(self, cap: ProofLevel) -> ProofLevel;
}
impl core::str::FromStr for ProofLevel { type Err = CoreError; }
impl core::fmt::Display for ProofLevel {}

pub const PROOF_LEVEL_KEY: &str = "apw:proof_level";

// ============================== verification states ==============================
/// The four provenance verification outcomes. This vocabulary is closed.
///
/// IMPORTANT: `NothingFound` records the absence of provenance data. It is NEVER
/// evidence that an asset is synthetic, machine-generated, or untrustworthy. A
/// caller that renders `NothingFound` as a synthetic-origin claim is misusing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationState {
    Verified,
    RegisteredButChanged,
    MarkFoundClaimNotTrusted,
    NothingFound,
}
impl VerificationState {
    pub const ALL: [VerificationState; 4];
    pub fn as_str(self) -> &'static str;   // "verified" | "registered_but_changed" | ...
    pub fn parse(value: &str) -> Result<VerificationState>;
}
pub const NOTHING_FOUND_NORMATIVE_NOTE: &str =
    "A missing mark is not proof of synthetic origin. NOTHING_FOUND records the \
     absence of provenance data, not the presence of a generation signal.";

/// The local POC manifest verifier's own outcome vocabulary. Distinct from
/// `VerificationState`: it grades one local manifest file, not a registry record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOutcome { Verified, Changed, Untrusted, NotFound, Incomplete }
impl LocalOutcome {
    pub fn as_str(self) -> &'static str;   // "verified" | "changed" | "untrusted" | "not_found" | "incomplete"
}

// ============================== findings ==============================
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity { Error, Warning, Info }

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Finding { pub severity: Severity, pub code: String, pub message: String }

#[derive(Debug, Clone, Default)]
pub struct VerificationReport { pub findings: Vec<Finding> }

impl VerificationReport {
    pub fn new() -> Self;
    pub fn error(&mut self, code: &str, message: impl Into<String>);
    pub fn warn(&mut self, code: &str, message: impl Into<String>);
    pub fn info(&mut self, code: &str, message: impl Into<String>);
    pub fn extend(&mut self, other: VerificationReport);
    pub fn complete(&mut self, check: &str);            // a REQUIRED_CHECKS entry ran
    pub fn unchecked(&self) -> Vec<(&'static str, &'static str)>;   // (check, reason) not yet run
    pub fn errors(&self) -> impl Iterator<Item = &Finding>;
    pub fn warnings(&self) -> impl Iterator<Item = &Finding>;
    pub fn passed(&self) -> bool;                       // no error findings
    /// Precedence, in order: any `not_found` code wins; then any CHANGED_CODES;
    /// then any error OR the `portable_signature_missing` warning; then any unrun
    /// REQUIRED_CHECKS entry (`incomplete`); else verified.
    pub fn outcome(&self) -> LocalOutcome;
    pub fn to_json(&self) -> serde_json::Value;         // shape of artifacts/*_verification.json
}

/// Codes that force `LocalOutcome::Changed` regardless of severity.
pub const CHANGED_CODES: [&str; 9] = [
    "export_hash_mismatch", "evidence_hash_mismatch", "evidence_truncated",
    "tampered", "signature_invalid", "portable_signature_invalid",
    "stem_commitment_mismatch", "c2pa_asset_hash_mismatch", "c2pa_hard_binding_broken",
];

/// `export_binding`, `evidence_binding`, `portable_signature`, `c2pa_claim`, each with the
/// reason a run that skipped it cannot claim `verified` (`daemon/verify.py::_REQUIRED_CHECKS`).
pub const REQUIRED_CHECKS: [(&str, &str); 4];

// ============================== canonical JSON ==============================
/// Two canonicalizations exist and they are NOT interchangeable. Both sort keys
/// and use `(",", ":")` separators; they differ only in non-ASCII handling, and
/// diverge the moment a track name, file name or .als string is non-ASCII.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Canonicalization {
    /// `apw-json-sort-v1`: raw UTF-8. Ed25519 portable signature, evidence JSONL,
    /// UDP acknowledgements, and the evidence-bundle index id.
    Utf8,
    /// `apw-json-sort-ascii-v1`: `\uXXXX`-escaped. The local HMAC
    /// `manifest_signature` input and the verifier's tamper recomputation.
    AsciiEscaped,
}
impl Canonicalization { pub fn label(self) -> &'static str; }

pub const CANONICALIZATION_PORTABLE: &str = "apw-json-sort-v1";
pub const CANONICALIZATION_LOCAL: &str = "apw-json-sort-ascii-v1";

/// REQUIRED: rejects non-finite floats (Python `allow_nan=False`); serde_json's
/// default would silently emit `null`. Formats every f64 with `python_repr_f64`.
pub fn canonical_json(value: &serde_json::Value, mode: Canonicalization) -> Result<Vec<u8>>;
pub fn canonical_json_utf8(value: &serde_json::Value) -> Result<Vec<u8>>;
pub fn canonical_json_ascii(value: &serde_json::Value) -> Result<Vec<u8>>;
pub fn canonical_json_of<T: serde::Serialize>(value: &T, mode: Canonicalization) -> Result<Vec<u8>>;

/// CPython `repr(float)` exactly: shortest round-tripping digits, fixed notation
/// for 1e-4 <= |x| < 1e16, otherwise scientific with a signed, zero-padded,
/// at-least-two-digit exponent. Verified divergences from ryu: `1e-07` vs `1e-7`,
/// `1e-05` vs `0.00001`, `3.0517578125e-05` vs `0.000030517578125`.
pub fn python_repr_f64(value: f64) -> Result<String>;

/// Shallow top-level key removal, matching the Python dict comprehensions that
/// build each signing input.
pub fn without_top_level_keys(value: &serde_json::Value, keys: &[&str]) -> serde_json::Value;

pub const PORTABLE_SIGNATURE_EXCLUDED_KEYS: [&str; 2] = ["portable_signature", "manifest_signature"];
pub const LOCAL_SIGNATURE_EXCLUDED_KEYS: [&str; 1] = ["manifest_signature"];

/// `json.dumps(..., indent=2, ensure_ascii=False)` + trailing newline.
pub fn pretty_json_bytes(value: &serde_json::Value) -> Result<Vec<u8>>;

// ============================== hashing ==============================
pub const HASH_CHUNK_BYTES: usize = 1 << 20;

pub fn sha256_hex(bytes: &[u8]) -> String;
pub fn sha256_file(path: &std::path::Path) -> Result<String>;
pub fn sha256_file_with_size(path: &std::path::Path) -> Result<(String, u64)>;
/// Hashes exactly `byte_length` bytes without loading the file. `ShortPrefix` if
/// the file ends first. Bounded: `byte_length` is attacker-supplied in a manifest.
pub fn sha256_prefix(path: &std::path::Path, byte_length: u64) -> Result<String>;
pub fn sha256_reader<R: std::io::Read>(reader: &mut R) -> Result<(String, u64)>;

// ============================== time ==============================
/// `datetime.fromtimestamp(t, utc).isoformat().replace("+00:00","Z")`:
/// microsecond field present only when non-zero, and then always 6 digits.
pub fn utc_timestamp(at: Option<std::time::SystemTime>) -> String;
/// `time.strftime("%Y-%m-%dT%H:%M:%SZ", gmtime())`: whole seconds, no fraction.
/// Used only for `export.exported_at`.
pub fn utc_timestamp_seconds(at: Option<std::time::SystemTime>) -> String;

// ============================== portable signature ==============================
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortableSignature {
    pub algorithm: String,               // "Ed25519"
    pub canonicalization: String,        // CANONICALIZATION_PORTABLE
    pub public_key_hex: String,
    pub public_key_file: String,
    pub signer_id: String,               // sha256(pk_raw)[..16] hex
    pub signature_hex: String,
    pub signed_content_hash: String,
    pub trust_scope: String,             // TRUST_SCOPE_SELF_GENERATED
    pub signer_identity: String,         // "not_established"
    pub signer_identity_proof_level: ProofLevel,   // always UnknownUnobserved
    #[serde(rename = "apw:proof_level")]
    pub proof_level: ProofLevel,         // DirectlyObserved (the signing act was observed)
    pub notes: String,
}

/// A self-issued key proves POSSESSION OF THE KEY. It does not establish identity.
/// The two are never merged into one field.
pub const TRUST_SCOPE_SELF_GENERATED: &str = "self_generated_demo_key_integrity";
pub const TRUST_SCOPE_LOCAL_SOFTWARE: &str = "local_software_integrity";
pub const TRUST_SCOPE_HARDWARE_PROVIDER: &str = "hardware_provider";

pub struct Ed25519Signer { /* private; key zeroized on drop */ }

impl Ed25519Signer {
    /// Loads a 32-byte raw seed, or generates and persists one at 0o600.
    /// Rewrites the raw public key at 0o644 when it is absent or stale.
    pub fn load_or_create(private_key_path: &std::path::Path,
                          public_key_path: &std::path::Path) -> Result<Self>;
    pub fn public_key_bytes(&self) -> [u8; 32];
    pub fn public_key_hex(&self) -> String;
    pub fn public_key_path(&self) -> &std::path::Path;
    pub fn signer_id(&self) -> String;
    /// Signs `canonical_json_utf8(unsigned)`. The caller is responsible for
    /// passing the correct exclusion set; see `PORTABLE_SIGNATURE_EXCLUDED_KEYS`.
    pub fn sign_manifest(&self, unsigned: &serde_json::Value) -> Result<PortableSignature>;
}

/// Rejection reasons render to the exact strings the Python verifier emits,
/// because they become `Finding::message` inside a hash-bound bundle member.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignatureRejection {
    #[error("portable public key is malformed")]            MalformedPublicKey,
    #[error("portable public key must be 32 raw bytes")]    PublicKeyWrongLength,
    #[error("portable signature is malformed")]             MalformedSignature,
    #[error("portable signed-content hash does not match canonical manifest")]
                                                            ContentHashMismatch,
    #[error("Ed25519 signature is invalid")]                 SignatureInvalid,
}

pub const PORTABLE_SIGNATURE_VALID_MESSAGE: &str =
    "Ed25519 signature verified with the public key; signer identity remains unverified";

pub fn verify_portable_signature(
    unsigned: &serde_json::Value,
    signature: &PortableSignature,
    public_key_override: Option<&[u8; 32]>,
) -> core::result::Result<&'static str, SignatureRejection>;

// IMPORTANT: Python also has a sixth failure path. `verify.py` wraps the call in
// `except (OSError, ValueError, ImportError)` and emits
// `f"Ed25519 verification failed: {exc}"` under code `portable_signature_invalid`.
// This API moves key-file loading out to the caller, so reproducing that message
// is the apw-daemon call site's obligation, not this function's.

// ============================== manifest domain types ==============================
pub const APW_VERSION: &str = "0.9.0";
pub const MANIFEST_SCHEMA: &str = "audio-provenance-manifest-v0";
pub const CORE_PRINCIPLE: &str = "Never claim full DAW provenance.";
pub const MAX_PROOF_VALUE_DEPTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus { CompleteObservedPath, PartialObservedPath, UnknownCoverage }

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationStatus { InferredMatch, NotEstablished, Unavailable }
impl AssociationStatus {
    /// INVARIANT (schema-enforced): InferredMatch pairs only with Inferred;
    /// NotEstablished and Unavailable pair only with UnknownUnobserved.
    pub fn required_proof_level(self) -> ProofLevel;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus { Issued, Degraded, Unknown }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemEvidence {
    pub stem_id: String,
    pub hash_chain_root: String,
    pub hash_chain_genesis: String,          // "genesis" for a clean first window
    pub hash_chain_length: u64,
    pub first_observed_ms: i64,
    pub last_observed_ms: i64,
    pub first_received_at: Option<String>,
    pub last_received_at: Option<String>,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub source_category: String,
    pub source_category_proof_level: ProofLevel,
    pub plugin_instance_ids: Vec<String>,
    pub proof_level: ProofLevel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportEvidence {
    pub file_path: String,
    pub file_name: String,
    pub sha256: String,
    pub format: String,
    pub file_size_bytes: u64,
    pub duration_seconds: Option<f64>,
    /// REQUIRED: Number, not f64. Python's `extract_audio_metadata` returns an
    /// `int` from the wave/aifc path and an `int`-or-`float` from the afinfo
    /// path, so `44100` and `44100.0` are different canonical bytes.
    pub sample_rate_hz: Option<serde_json::Number>,
    pub channel_count: Option<u16>,
    pub exported_at: String,
    pub export_version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngredientEvidence {
    pub file_name: String,
    pub sha256: String,
    pub proof_level: ProofLevel,
    pub correlation_confidence: Option<f64>,
    pub audio_fingerprint: Option<serde_json::Value>,
}

/// Insertion-ordered manifest document. Order is cosmetic (the pretty file);
/// both signing inputs sort keys, so a reordering never breaks a signature.
///
/// INVARIANT: the inner Value is always `Value::Object`; `from_value` is the only
/// constructor and rejects anything else.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest(serde_json::Value);

impl Manifest {
    pub fn from_value(value: serde_json::Value) -> Result<Self>;
    pub fn as_value(&self) -> &serde_json::Value;
    pub fn as_object(&self) -> &serde_json::Map<String, serde_json::Value>;
    pub fn get(&self, key: &str) -> Option<&serde_json::Value>;
    pub fn get_mut(&mut self, key: &str) -> Option<&mut serde_json::Value>;
    /// Python dict semantics: a new key appends, an existing key keeps its slot.
    pub fn insert(&mut self, key: &str, value: serde_json::Value) -> Option<serde_json::Value>;
    pub fn session_id(&self) -> Option<&str>;
    /// canonical_json_utf8 over the document minus PORTABLE_SIGNATURE_EXCLUDED_KEYS.
    pub fn portable_signing_input(&self) -> Result<Vec<u8>>;
    /// canonical_json_ascii over the document minus LOCAL_SIGNATURE_EXCLUDED_KEYS.
    /// IMPORTANT: this input includes `portable_signature`, so the portable
    /// signature MUST be attached before the local signature is computed.
    pub fn local_signing_input(&self) -> Result<Vec<u8>>;
    pub fn to_pretty_bytes(&self) -> Result<Vec<u8>>;
    pub fn write_pretty(&self, path: &std::path::Path) -> Result<()>;
}

#[derive(Debug, Clone, Default)]
pub struct ManifestBuilder { /* private */ }

impl ManifestBuilder {
    pub fn new(session_id: impl Into<String>, created_at: String) -> Self;
    pub fn add_stem(&mut self, stem: StemEvidence) -> &mut Self;
    pub fn set_export(&mut self, export: ExportEvidence) -> &mut Self;
    pub fn add_ingredient(&mut self, ingredient: IngredientEvidence) -> &mut Self;
    pub fn add_composite_edit(&mut self, edit: serde_json::Value) -> &mut Self;
    pub fn set_coverage(&mut self, coverage: serde_json::Value) -> &mut Self;
    pub fn set_audio_association(&mut self, association: serde_json::Value) -> &mut Self;
    pub fn set_forgery_report(&mut self, report: serde_json::Value) -> &mut Self;
    pub fn set_session_diagnostics(&mut self, diagnostics: serde_json::Value) -> &mut Self;
    pub fn set_hardware_binding(&mut self, binding: serde_json::Value) -> &mut Self;
    /// The real signed C2PA claim record produced by apw-c2pa. Absent, `build`
    /// emits `unavailable_c2pa_claim("No C2PA claim was supplied to the builder.")`.
    pub fn set_c2pa_claim(&mut self, claim: serde_json::Value) -> &mut Self;
    pub fn add_time_anchor(&mut self, anchor: serde_json::Value) -> &mut Self;
    pub fn unobserved_mut(&mut self) -> &mut Vec<String>;
    pub fn build(&self) -> Result<Manifest>;
}

/// `c2pa_mapping` is a descriptive projection; `c2pa_claim` is the signed claim.
/// They are not interchangeable and neither substitutes for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum C2paClaimStatus { Embedded, Sidecar, Unavailable }
impl C2paClaimStatus {
    pub fn is_signed(self) -> bool;                  // Embedded | Sidecar
    pub fn claim_proof_level(self) -> ProofLevel;    // DirectlyObserved when signed
}

pub const C2PA_CLAIM_SCOPE: &str =
    "A real C2PA claim signed with a locally issued X.509 chain and bound to the \
     asset by a SHA-256 hard binding. The signing act and the binding are directly \
     observed. The signer identity is self-asserted: a 'verified' validation state \
     means the claim chains to this machine's own root certificate, not to any \
     external trust list, registry, or verified creator identity.";

pub fn unavailable_c2pa_claim(reason: &str) -> serde_json::Value;

pub const DEFAULT_UNOBSERVED: [&str; 5] = [
    "hidden_plugin_state", "internal_preset_logic", "bypassed_routing",
    "daw_internal_processing", "unverifiable_upstream_provenance",
];

// ============================== schema invariants ==============================
/// Returns the enforced-invariant violations in the same order and wording the
/// Python validator produces; each one becomes a `schema_invalid` error finding.
/// Bounded recursion: `NestingTooDeep` beyond MAX_PROOF_VALUE_DEPTH is reported
/// as a violation, never a stack overflow (untrusted verifier input).
pub fn validate_manifest_invariants(value: &serde_json::Value) -> Vec<String>;

pub const REQUIRED_MANIFEST_KEYS: [&str; 13] = [
    "apw_version", "schema", "session_id", "capture_session", "created_at",
    "observed_stems", "claim_summary", "stem_export_association",
    "observation_coverage", "daemon_receipt_acknowledgement",
    "apw:unobserved", "c2pa_mapping", "c2pa_claim",
];

// ============================== evidence log ==============================
pub const DEFAULT_EVIDENCE_MAX_BYTES: u64 = 64 * 1024 * 1024;
pub const DEFAULT_EVIDENCE_BACKUPS: u32 = 3;

/// Appends one `canonical_json_utf8` record plus `\n`, rotating at the cap.
/// An oversize record is dropped and logged, never truncated into the file.
pub fn append_jsonl(path: &std::path::Path, record: &serde_json::Value,
                    max_bytes: u64, backup_count: u32) -> Result<()>;
/// Existing rotations oldest-first, then the active file.
pub fn rotated_evidence_paths(path: &std::path::Path) -> Result<Vec<std::path::PathBuf>>;
```
