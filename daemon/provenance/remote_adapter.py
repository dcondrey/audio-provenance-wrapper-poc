from __future__ import annotations

import logging
from pathlib import Path

from daemon.provenance.provider import ProvenanceProvider, SigningMaterial

log = logging.getLogger(__name__)

DEFAULT_BASE_URL = "https://provenance.invalid/v1"


class RemoteProvenanceProvider(ProvenanceProvider):
    """Seam for a hosted vault/mark/registry service that does not exist yet.

    Every method raises NotImplementedError naming exactly what the service must
    expose for that method to work. This class is the API contract: the endpoint
    shape, auth model, request and response fields, and failure modes we need.

    Auth model assumed throughout: OAuth 2.0 client credentials over TLS 1.3,
    scoped bearer token in `Authorization: Bearer <token>`, one scope per
    capability group (vault:read, vault:sign, vault:admin, mark:write,
    mark:read, registry:write, registry:read). Every request carries an
    `Idempotency-Key` header; every response carries `X-Request-Id`.
    All calls are non-realtime and MUST NOT run on the audio thread.
    """

    def __init__(self, base_url: str = DEFAULT_BASE_URL, client_id: str | None = None) -> None:
        self.base_url = base_url
        self.client_id = client_id
        log.warning(
            "RemoteProvenanceProvider is a contract stub; no remote service is reachable at %s",
            base_url,
        )

    def identity(self) -> dict:
        raise NotImplementedError(
            "Needs GET {base}/identity -> 200 {key_id, subject_common_name, algorithm, "
            "identity_evidence, identity_proof_level in (user_declared|externally_verified), "
            "verifying_authority, verified_at, revoked, revoked_at|null, trust_anchor_sha256}. "
            "Scope vault:read. identity_proof_level MUST report how the service verified the "
            "creator, not merely that a key exists; we map it straight onto apw:proof_level and "
            "must never upgrade it locally. Failure modes to define: 401 invalid token, "
            "403 wrong scope, 404 no identity provisioned for this client, 410 identity retired."
        )

    def issue_signing_material(self) -> SigningMaterial:
        raise NotImplementedError(
            "Needs POST {base}/vault/signing-material {key_id?, purpose:\"c2pa_claim\"} -> 201 "
            "{key_id, algorithm:\"es256\", certificate_chain_pem (leaf-then-issuer, CA:FALSE leaf "
            "with critical digitalSignature and critical emailProtection EKU, issuer CA:TRUE with "
            "critical keyCertSign), trust_anchor_pem, key_handle, expires_at}. Scope vault:sign. "
            "The private key MUST stay in the vault: key_handle is an opaque reference used by "
            "sign_claim, never key bytes. A self-signed leaf is rejected by the C2PA verifier, so "
            "a full chain is mandatory. Failure modes: 403 key revoked, 409 no valid cert (needs "
            "enrolment), 423 key locked pending recovery, 503 HSM unavailable."
        )

    def sign_claim(self, payload: bytes) -> bytes:
        raise NotImplementedError(
            "Needs POST {base}/vault/sign {key_handle, alg:\"es256\", payload_b64, "
            "payload_sha256} -> 200 {signature_b64 (raw r||s, 64 bytes for P-256, not DER), "
            "key_id, signed_at, history_entry_id}. Scope vault:sign. The service MUST record the "
            "signature in the key's immutable signing history before returning, and MUST return "
            "the resulting history_entry_id so we can cite it in evidence. Failure modes: "
            "403 key revoked (MUST NOT sign), 409 payload_sha256 disagrees with payload_b64, "
            "413 payload over the size limit, 429 rate limit with Retry-After."
        )

    def embed_mark(self, audio_path: Path, payload: dict) -> dict:
        raise NotImplementedError(
            "Needs POST {base}/mark/embed (multipart: audio file + json payload) -> 200 "
            "{mark_id, marked_asset_url or marked_asset_b64, asset_modified:true, algorithm_id, "
            "algorithm_version, payload_capacity_bits, measured_transparency (the actual listening "
            "or PEAQ/ODG result), measured_survival (a matrix of codec/bitrate/operation -> "
            "recovery rate, with sample counts)}. Scope mark:write. We will publish "
            "measured_transparency and measured_survival verbatim and will not restate them as "
            "claims of inaudibility or transcode survival unless the service supplies the "
            "measurements. Also needs the supported input formats and sample rates. Failure modes: "
            "415 unsupported audio format, 422 asset too short to carry the payload, "
            "413 file over the size limit."
        )

    def recover_mark(self, audio_path: Path) -> dict | None:
        raise NotImplementedError(
            "Needs POST {base}/mark/recover (multipart: audio file) -> 200 "
            "{found:bool, mark_id?, payload?, confidence (0-1), false_positive_rate_at_confidence, "
            "detector_version} and 200 {found:false} for a clean miss. Scope mark:read. A miss MUST "
            "be a 200 with found:false, never a 404 that we could confuse with a transport error, "
            "and MUST NOT be reported as evidence of synthetic origin. "
            "false_positive_rate_at_confidence is required: without it a confidence number is not "
            "actionable. Failure modes: 415 unsupported format, 503 detector unavailable."
        )

    def register(self, manifest: dict) -> dict:
        raise NotImplementedError(
            "Needs POST {base}/registry/manifests {manifest (C2PA manifest bytes b64 or JSON), "
            "content_sha256, mark_id?, key_id} -> 201 {registry_id, content_sha256, "
            "manifest_sha256, registered_at, receipt_signature_b64, receipt_signing_chain_pem, "
            "federation_peers[]} plus GET {base}/registry/manifests?content_sha256= and "
            "?mark_id= for content-addressed lookup. Scope registry:write / registry:read. The "
            "receipt MUST be independently verifiable offline (signature over "
            "registry_id||content_sha256||manifest_sha256||registered_at) so a verifier need not "
            "trust the transport. Records MUST be append-only: define whether a superseding record "
            "is a new registry_id and how a verifier walks the chain. Failure modes: "
            "409 already registered (return the existing receipt, do not error), "
            "422 manifest fails C2PA validation, 402 quota exceeded."
        )

    def verify(self, asset_path: Path, manifest_bytes: bytes | None) -> dict:
        raise NotImplementedError(
            "Needs POST {base}/verify (multipart: audio file + optional manifest bytes) -> 200 "
            "{state in (verified|registered_but_changed|mark_found_claim_not_trusted|"
            "nothing_found), reason, content_sha256, registry_id?, mark?, signer{key_id, "
            "subject_common_name, identity_proof_level, revoked}, c2pa_validation_state, "
            "c2pa_failures[]}. Scope registry:read. The state vocabulary MUST be exactly those "
            "four values. nothing_found MUST be documented as the absence of provenance data and "
            "never as an assertion of synthetic origin. The response MUST distinguish an untrusted "
            "signer from altered content, since those are different remediations. Failure modes: "
            "415 unsupported format, 503 registry unreachable (which is NOT nothing_found and MUST "
            "surface as an error, not a verdict)."
        )

    def revoke(self, key_id: str) -> dict:
        raise NotImplementedError(
            "Needs POST {base}/vault/keys/{key_id}/revoke {reason, effective_at} -> 200 "
            "{key_id, revoked:true, revoked_at, retained_signing_records:int, "
            "signatures_before_revocation_remain_valid:bool}. Scope vault:admin. Revocation MUST "
            "mark the key unusable for new signatures while retaining every prior signing record: "
            "erasing what a creator signed is not this operation, and the service must state "
            "whether pre-revocation signatures stay valid (effective_at semantics). Also needs the "
            "recovery counterpart POST {base}/vault/keys/{key_id}/recover so a creator can regain a "
            "verified identity without losing history. Failure modes: 409 already revoked, "
            "403 caller not the key owner, 423 pending recovery hold."
        )

    def signing_history(self, key_id: str) -> list[dict]:
        raise NotImplementedError(
            "Needs GET {base}/vault/keys/{key_id}/history?cursor=&limit= -> 200 "
            "{entries:[{history_entry_id, signed_at, payload_sha256, signature_b64, "
            "content_sha256?, registry_id?}], next_cursor, total, log_inclusion_proof?}. Scope "
            "vault:read. History MUST remain readable after revocation, MUST be append-only, and "
            "SHOULD carry a transparency-log inclusion proof so a third party can confirm nothing "
            "was removed. Failure modes: 404 unknown key_id, 403 caller not the key owner."
        )
