use std::path::Path;

use serde_json::Value;

use crate::error::{ProvenanceError, Result};
use crate::provider::{
    IdentityReport, MarkAttachment, ProvenanceProvider, RecoveredMark, RegistryReceipt,
    RevocationRecord, SigningMaterial, SigningRecord, VerificationOutcome,
};

pub const DEFAULT_BASE_URL: &str = "https://provenance.invalid/v1";

/// Auth model assumed by every requirement below: OAuth 2.0 client credentials
/// over TLS 1.3, scoped bearer token in `Authorization: Bearer <token>`, one scope
/// per capability group. Every request carries an `Idempotency-Key` header; every
/// response carries `X-Request-Id`. All calls are non-realtime and MUST NOT run on
/// the audio thread.
pub const REMOTE_AUTH_MODEL: &str = "OAuth 2.0 client credentials over TLS 1.3; scoped bearer \
     token in `Authorization: Bearer <token>`; scopes vault:read, vault:sign, vault:admin, \
     mark:write, mark:read, registry:write, registry:read; `Idempotency-Key` on every request; \
     `X-Request-Id` on every response";

/// One endpoint a hosted provenance service must expose.
///
/// This type IS the API contract. Each field is what an implementer needs before
/// the corresponding [`RemoteProvenanceProvider`] method can do anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteRequirement {
    pub operation: &'static str,
    pub method: &'static str,
    pub endpoint: &'static str,
    pub scope: &'static str,
    pub request: &'static str,
    pub response: &'static str,
    pub failure_modes: &'static str,
    pub constraints: &'static str,
}

impl core::fmt::Display for RemoteRequirement {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "  operation:  {}", self.operation)?;
        writeln!(f, "  endpoint:   {} {}", self.method, self.endpoint)?;
        writeln!(f, "  auth:       {} (scope {})", REMOTE_AUTH_MODEL, self.scope)?;
        writeln!(f, "  request:    {}", self.request)?;
        writeln!(f, "  response:   {}", self.response)?;
        writeln!(f, "  failures:   {}", self.failure_modes)?;
        write!(f, "  constraints:{}", self.constraints)
    }
}

pub const IDENTITY_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "identity",
    method: "GET",
    endpoint: "{base}/identity",
    scope: "vault:read",
    request: "(none)",
    response: "200 {key_id, subject_common_name, algorithm, identity_evidence, \
        identity_proof_level in (user_declared|externally_verified), verifying_authority, \
        verified_at, revoked, revoked_at|null, trust_anchor_sha256}",
    failure_modes: "401 invalid token, 403 wrong scope, 404 no identity provisioned for this \
        client, 410 identity retired",
    constraints: " identity_proof_level MUST report how the service verified the creator, not \
        merely that a key exists; it maps straight onto apw:proof_level and MUST NOT be upgraded \
        locally. A self-issued chain proves key possession, not verified identity.",
};

pub const SIGNING_MATERIAL_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "issue_signing_material",
    method: "POST",
    endpoint: "{base}/vault/signing-material",
    scope: "vault:sign",
    request: "{key_id?, purpose:\"c2pa_claim\"}",
    response: "201 {key_id, algorithm:\"es256\", certificate_chain_pem (leaf-then-issuer, \
        CA:FALSE leaf with critical digitalSignature and critical emailProtection EKU, issuer \
        CA:TRUE with critical keyCertSign), trust_anchor_pem, key_handle, expires_at}",
    failure_modes: "403 key revoked, 409 no valid cert (needs enrolment), 423 key locked pending \
        recovery, 503 HSM unavailable",
    constraints: " The private key MUST stay in the vault: key_handle is an opaque reference used \
        by sign_claim, never key bytes. A self-signed leaf is rejected by the C2PA verifier, so a \
        full chain is mandatory.",
};

pub const SIGN_CLAIM_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "sign_claim",
    method: "POST",
    endpoint: "{base}/vault/sign",
    scope: "vault:sign",
    request: "{key_handle, alg:\"es256\", payload_b64, payload_sha256}",
    response: "200 {signature_b64 (raw r||s, 64 bytes for P-256, not DER), key_id, signed_at, \
        history_entry_id}",
    failure_modes: "403 key revoked (MUST NOT sign), 409 payload_sha256 disagrees with \
        payload_b64, 413 payload over the size limit, 429 rate limit with Retry-After",
    constraints: " The service MUST record the signature in the key's immutable signing history \
        before returning, and MUST return the resulting history_entry_id so it can be cited in \
        evidence.",
};

pub const EMBED_MARK_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "embed_mark",
    method: "POST",
    endpoint: "{base}/mark/embed",
    scope: "mark:write",
    request: "multipart: audio file + json payload; also needs the documented list of supported \
        input formats and sample rates",
    response: "200 {mark_id, marked_asset_url or marked_asset_b64, asset_modified:true, \
        algorithm_id, algorithm_version, payload_capacity_bits, measured_transparency (the actual \
        listening or PEAQ/ODG result), measured_survival (a matrix of codec/bitrate/operation -> \
        recovery rate, with sample counts)}",
    failure_modes: "415 unsupported audio format, 422 asset too short to carry the payload, \
        413 file over the size limit",
    constraints: " measured_transparency and measured_survival are published verbatim and are \
        never restated as claims of inaudibility or transcode survival unless the service supplies \
        the measurements.",
};

pub const RECOVER_MARK_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "recover_mark",
    method: "POST",
    endpoint: "{base}/mark/recover",
    scope: "mark:read",
    request: "multipart: audio file",
    response: "200 {found:bool, mark_id?, payload?, confidence (0-1), \
        false_positive_rate_at_confidence, detector_version}; a clean miss is 200 {found:false}",
    failure_modes: "415 unsupported format, 503 detector unavailable",
    constraints: " A miss MUST be a 200 with found:false, never a 404 that could be confused with \
        a transport error, and MUST NOT be reported as evidence of synthetic origin. \
        false_positive_rate_at_confidence is required: without it a confidence number is not \
        actionable.",
};

pub const REGISTER_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "register",
    method: "POST",
    endpoint: "{base}/registry/manifests (plus GET ?content_sha256= and ?mark_id=)",
    scope: "registry:write / registry:read",
    request: "{manifest (C2PA manifest bytes b64 or JSON), content_sha256, mark_id?, key_id}",
    response: "201 {registry_id, content_sha256, manifest_sha256, registered_at, \
        receipt_signature_b64, receipt_signing_chain_pem, federation_peers[]}",
    failure_modes: "409 already registered (return the existing receipt, do not error), \
        422 manifest fails C2PA validation, 402 quota exceeded",
    constraints: " The receipt MUST be independently verifiable offline (signature over \
        registry_id||content_sha256||manifest_sha256||registered_at) so a verifier need not trust \
        the transport. Records MUST be append-only: define whether a superseding record is a new \
        registry_id and how a verifier walks the chain.",
};

pub const VERIFY_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "verify",
    method: "POST",
    endpoint: "{base}/verify",
    scope: "registry:read",
    request: "multipart: audio file + optional manifest bytes",
    response: "200 {state in (verified|registered_but_changed|mark_found_claim_not_trusted|\
        nothing_found), reason, content_sha256, registry_id?, mark?, signer{key_id, \
        subject_common_name, identity_proof_level, revoked}, c2pa_validation_state, \
        c2pa_failures[]}",
    failure_modes: "415 unsupported format, 503 registry unreachable, which is NOT nothing_found \
        and MUST surface as an error rather than a verdict",
    constraints: " The state vocabulary MUST be exactly those four values. nothing_found MUST be \
        documented as the absence of provenance data and never as an assertion of synthetic \
        origin. The response MUST distinguish an untrusted signer from altered content, since \
        those are different remediations.",
};

pub const REVOKE_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "revoke",
    method: "POST",
    endpoint: "{base}/vault/keys/{key_id}/revoke (with a recovery counterpart at \
        {base}/vault/keys/{key_id}/recover)",
    scope: "vault:admin",
    request: "{reason, effective_at}",
    response: "200 {key_id, revoked:true, revoked_at, retained_signing_records:int, \
        signatures_before_revocation_remain_valid:bool}",
    failure_modes: "409 already revoked, 403 caller not the key owner, 423 pending recovery hold",
    constraints: " Revocation MUST mark the key unusable for new signatures while retaining every \
        prior signing record: erasing what a creator signed is not this operation. The service \
        must state whether pre-revocation signatures stay valid (effective_at semantics), and must \
        offer recovery so a creator can regain a verified identity without losing history.",
};

pub const SIGNING_HISTORY_REQUIREMENT: RemoteRequirement = RemoteRequirement {
    operation: "signing_history",
    method: "GET",
    endpoint: "{base}/vault/keys/{key_id}/history?cursor=&limit=",
    scope: "vault:read",
    request: "(none)",
    response: "200 {entries:[{history_entry_id, signed_at, payload_sha256, signature_b64, \
        content_sha256?, registry_id?}], next_cursor, total, log_inclusion_proof?}",
    failure_modes: "404 unknown key_id, 403 caller not the key owner",
    constraints: " History MUST remain readable after revocation, MUST be append-only, and SHOULD \
        carry a transparency-log inclusion proof so a third party can confirm nothing was removed.",
};

/// Every endpoint the hosted service must expose, in trait order.
pub const REMOTE_REQUIREMENTS: [RemoteRequirement; 9] = [
    IDENTITY_REQUIREMENT,
    SIGNING_MATERIAL_REQUIREMENT,
    SIGN_CLAIM_REQUIREMENT,
    EMBED_MARK_REQUIREMENT,
    RECOVER_MARK_REQUIREMENT,
    REGISTER_REQUIREMENT,
    VERIFY_REQUIREMENT,
    REVOKE_REQUIREMENT,
    SIGNING_HISTORY_REQUIREMENT,
];

/// Seam for a hosted vault/mark/registry service that does not exist yet.
///
/// Every method returns [`ProvenanceError::RemoteServiceMissing`] carrying the
/// [`RemoteRequirement`] for that operation: endpoint shape, auth model, request
/// and response fields, and failure modes. This type is the API contract.
#[derive(Debug, Clone)]
pub struct RemoteProvenanceProvider {
    base_url: String,
    client_id: Option<String>,
}

impl Default for RemoteProvenanceProvider {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL, None)
    }
}

impl RemoteProvenanceProvider {
    pub fn new(base_url: &str, client_id: Option<&str>) -> Self {
        log::warn!(
            "RemoteProvenanceProvider is a contract stub; no remote service is reachable at {base_url}"
        );
        RemoteProvenanceProvider {
            base_url: base_url.to_string(),
            client_id: client_id.map(str::to_string),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn client_id(&self) -> Option<&str> {
        self.client_id.as_deref()
    }

    fn missing<T>(&self, requirement: &'static RemoteRequirement) -> Result<T> {
        Err(ProvenanceError::RemoteServiceMissing(requirement))
    }
}

impl ProvenanceProvider for RemoteProvenanceProvider {
    fn identity(&self) -> Result<IdentityReport> {
        self.missing(&IDENTITY_REQUIREMENT)
    }

    fn issue_signing_material(&self) -> Result<SigningMaterial> {
        self.missing(&SIGNING_MATERIAL_REQUIREMENT)
    }

    fn sign_claim(&self, _payload: &[u8]) -> Result<Vec<u8>> {
        self.missing(&SIGN_CLAIM_REQUIREMENT)
    }

    fn embed_mark(&self, _audio_path: &Path, _payload: &Value) -> Result<MarkAttachment> {
        self.missing(&EMBED_MARK_REQUIREMENT)
    }

    fn recover_mark(&self, _audio_path: &Path) -> Result<Option<RecoveredMark>> {
        self.missing(&RECOVER_MARK_REQUIREMENT)
    }

    fn register(&self, _manifest: &Value) -> Result<RegistryReceipt> {
        self.missing(&REGISTER_REQUIREMENT)
    }

    fn verify(
        &self,
        _asset_path: &Path,
        _manifest_bytes: Option<&[u8]>,
    ) -> Result<VerificationOutcome> {
        self.missing(&VERIFY_REQUIREMENT)
    }

    fn revoke(&self, _key_id: &str) -> Result<RevocationRecord> {
        self.missing(&REVOKE_REQUIREMENT)
    }

    fn signing_history(&self, _key_id: &str) -> Result<Vec<SigningRecord>> {
        self.missing(&SIGNING_HISTORY_REQUIREMENT)
    }
}
