# Provider Contract

## What this is

There is no public developer surface for the platform this adapter is meant to
feed: no SDK, no API, no package, no published spec, and every developer
subdomain is NXDOMAIN as of 31 August 2026. Rather than claim an integration that
cannot be demonstrated, we wrote the interface such a platform needs to expose and
a working local implementation behind it.

Nine capabilities, one abstract base class, two implementations:

| file | role |
|---|---|
| `daemon/provenance/provider.py` | `ProvenanceProvider` ABC, `VerificationState`, `SigningIdentity`, `SigningMaterial` |
| `daemon/provenance/local_reference.py` | `LocalReferenceProvider`, file-backed, operational today |
| `daemon/provenance/remote_adapter.py` | `RemoteProvenanceProvider`, the HTTP contract; every method raises `NotImplementedError` naming its requirement |
| `daemon/provenance/__init__.py` | `detect_provider()`, selects on `APW_PROVENANCE_PROVIDER` |

**What "give me an endpoint" actually means.** Provider selection is already a
configuration change: `APW_PROVENANCE_PROVIDER=remote` returns
`RemoteProvenanceProvider`. Fixed and not up for renegotiation: the nine method
signatures, the four-value state vocabulary, the `apw:proof_level` discipline, and
the call sites. What remains is one HTTP client behind nine methods whose request
and response shapes are already specified below.

Not yet plumbed, and we are not pretending otherwise: `detect_provider` constructs
`RemoteProvenanceProvider()` with no arguments, so `base_url` and `client_id` are
constructor parameters with no environment wiring. A few lines, not a design
question, but not done.

## What the export pipeline actually calls

Read this before the capability sections. All nine capabilities are implemented
and tested; only two are wired into the export path today.

| capability | invoked at export? |
|---|---|
| `issue_signing_material`, `identity` | **Yes**, from `daemon/manifest_builder/generator.py` when the C2PA claim is produced |
| `sign_claim` | **No.** The C2PA claim is signed by c2pa-rs from the issued material, not through this method; `sign_claim` is reached only from `register()` |
| `embed_mark`, `recover_mark`, `register`, `verify`, `revoke`, `signing_history` | **No.** Operational and covered by tests, with no call site in the export pipeline |

The local registry and soft binding are therefore a working reference for the
contract, not a shipping feature of the demo. The demo's verification path is the
C2PA grader described under "The four verification states".

## Auth model

Assumed throughout the remote contract: OAuth 2.0 client credentials over TLS 1.3,
scoped bearer token in `Authorization: Bearer <token>`, one scope per capability
group (`vault:read`, `vault:sign`, `vault:admin`, `mark:write`, `mark:read`,
`registry:write`, `registry:read`). Every request carries `Idempotency-Key`; every
response carries `X-Request-Id`. Every call is non-realtime and none of it may run
on the audio thread.

## Capabilities

### 1. Identity

**Today.** `identity()` returns key_id, subject CN, `es256`, `identity_evidence:
locally_generated_key_no_external_attestation`, revoked flag, trust anchor
SHA-256, `apw:proof_level: user_declared`, and a `limits` string stating that no
external party verified the name.

**Remote must expose.** `GET {base}/identity` → 200 `{key_id,
subject_common_name, algorithm, identity_evidence, identity_proof_level ∈
(user_declared|externally_verified), verifying_authority, verified_at, revoked,
revoked_at|null, trust_anchor_sha256}`. Scope `vault:read`. Fail: 401 token, 403
scope, 404 no identity provisioned for this client, 410 identity retired.
`identity_proof_level` must report how the service verified the creator, not
merely that a key exists.

**Not claimed.** That the subject common name belongs to the human operating the
DAW. Nobody attested to it.

### 2. Signing material issuance

**Today.** `issue_signing_material()` returns a leaf-then-root PEM chain, the leaf
private key bytes, and the root as trust anchor. Raises `RevokedKeyError` if the
active key is revoked. The store self-heals: an expired, unreadable, or revoked
leaf is re-issued when the store opens rather than handed to a signer that will
refuse it.

**Remote must expose.** `POST {base}/vault/signing-material {key_id?,
purpose:"c2pa_claim"}` → 201 `{key_id, algorithm:"es256", certificate_chain_pem,
trust_anchor_pem, key_handle, expires_at}`. Scope `vault:sign`. The chain profile
is mandatory: leaf CA:FALSE with critical digitalSignature, critical
emailProtection EKU, and an authorityKeyIdentifier matching the issuer; issuer
CA:TRUE with critical keyCertSign. A self-signed leaf is rejected by the C2PA
verifier, so a full chain is not optional. The private key must stay in the vault:
`key_handle` is an opaque reference used by `sign_claim`, never key bytes. Fail:
403 key revoked, 409 no valid cert (enrolment needed), 423 locked pending
recovery, 503 HSM unavailable.

**Not claimed.** Hardware key custody. The local reference writes raw PKCS#8 key
bytes at mode 0600 and hands them back. `private_key_handle` is typed opaque
precisely so a vault can return a reference instead, but locally it is the key.

### 3. Claim signing

**Today.** `sign_claim(payload)` signs with ECDSA P-256 / SHA-256, then appends
`{event:"sign", key_id, at, payload_sha256, signature_hex, apw:proof_level:
directly_observed}` to `history.jsonl` before returning.

**Remote must expose.** `POST {base}/vault/sign {key_handle, alg:"es256",
payload_b64, payload_sha256}` → 200 `{signature_b64, key_id, signed_at,
history_entry_id}`. Signature is raw r‖s, 64 bytes for P-256, **not DER**. The
service must write the history entry before returning and must return its id so we
can cite it in evidence. Scope `vault:sign`. Fail: 403 key revoked (must not
sign), 409 `payload_sha256` disagrees with `payload_b64`, 413 oversize, 429 with
`Retry-After`.

**Not claimed.** A trusted timestamp. `at` is this machine's clock; no TSA is
involved.

### 4. Mark embed

**Today.** `embed_mark(path, payload)` computes a quantised feature descriptor,
HMACs it with a local 32-byte key together with the payload and content digest,
and appends the record to `marks.jsonl`. It returns `asset_modified: False`, the
mechanism in plain words, `apw:proof_level: inferred`, and an explicit limits
list. Mechanism and limits are in "The soft binding" below.

**Remote must expose.** `POST {base}/mark/embed` (multipart: audio file + JSON
payload) → 200 `{mark_id, marked_asset_url|marked_asset_b64, asset_modified:true,
algorithm_id, algorithm_version, payload_capacity_bits, measured_transparency,
measured_survival}`. Scope `mark:write`. `measured_transparency` is the actual
listening or PEAQ/ODG result; `measured_survival` is a codec/bitrate/operation →
recovery-rate matrix with sample counts. Also needed: supported input formats and
sample rates. Fail: 415 unsupported format, 422 asset too short to carry the
payload, 413 oversize.

**Not claimed.** Inaudibility, psychoacoustic masking, or watermarking of any
kind. No audio sample is modified. We will publish the service's
`measured_transparency` and `measured_survival` verbatim and will restate neither
as our own claim.

### 5. Mark recovery

**Today.** `recover_mark(path)` re-derives the descriptor and scans the side index
for the best match, requiring at least 90 % of dimensions to agree within ±1
bucket. Returns registered and observed content digests, similarity,
`apw:proof_level: inferred`, or `None`.

**Remote must expose.** `POST {base}/mark/recover` (multipart: audio file) → 200
`{found:bool, mark_id?, payload?, confidence, false_positive_rate_at_confidence,
detector_version}`. A miss must be 200 with `found:false`, never a 404 we could
confuse with a transport error. `false_positive_rate_at_confidence` is required: a
bare confidence number is not actionable. Scope `mark:read`. Fail: 415 format, 503
detector unavailable.

**Not claimed.** That a miss means anything about origin. Recovery needs this
machine's `marks.jsonl`; the mark does not travel with the asset.

### 6. Registration

**Today.** `register(manifest)` requires a 64-character `content_sha256`, signs
the canonical manifest bytes, builds a receipt, then signs the receipt's own
fields (`registry_id`, `content_sha256`, `manifest_sha256`, `mark_id`, `key_id`,
`registered_at`) under the domain tag `apw-registry-receipt-v1`, and appends it to
`registry.jsonl`. The receipt signature is what stops a hand-appended record that
carries a chain copied from a genuine one.

**Remote must expose.** `POST {base}/registry/manifests {manifest,
content_sha256, mark_id?, key_id}` → 201 `{registry_id, content_sha256,
manifest_sha256, registered_at, receipt_signature_b64, receipt_signing_chain_pem,
federation_peers[]}`, plus `GET {base}/registry/manifests?content_sha256=` and
`?mark_id=` for content-addressed lookup. Scopes `registry:write` /
`registry:read`. The receipt must verify offline over
`registry_id‖content_sha256‖manifest_sha256‖registered_at` so a verifier need not
trust the transport. Records must be append-only, and the service must define
whether a superseding record is a new `registry_id` and how a verifier walks the
chain. Fail: 409 already registered (**return the existing receipt, do not
error**), 422 manifest fails C2PA validation, 402 quota exceeded.

**Not claimed.** Federation, replication, or any reader outside this machine.
`registry.jsonl` is a local text file.

### 7. Verification

**Today.** Two graders emit the same four state strings from different evidence.
See "The four verification states" below.

**Remote must expose.** `POST {base}/verify` (multipart: audio file + optional
manifest bytes) → 200 `{state, reason, content_sha256, registry_id?, mark?,
signer{key_id, subject_common_name, identity_proof_level, revoked},
c2pa_validation_state, c2pa_failures[]}`. Scope `registry:read`. The state
vocabulary must be exactly the four values. The response must distinguish an
untrusted signer from altered content, since those are different remediations.
Fail: 415 format, 503 registry unreachable, **which is not `nothing_found` and
must surface as an error, not a verdict**.

**Not claimed.** An identity verdict. Our `verified` means the chain terminates at
a root this machine generated.

### 8. Revocation

**Today.** `revoke(key_id)` sets `revoked` and `revoked_at` in the key's
`meta.json`, appends a revoke event carrying `retained_signing_records`, and
leaves prior history intact. `sign_claim` and `issue_signing_material` then raise
`RevokedKeyError`; a replacement leaf is issued the next time the store opens.

**Remote must expose.** `POST {base}/vault/keys/{key_id}/revoke {reason,
effective_at}` → 200 `{key_id, revoked:true, revoked_at,
retained_signing_records, signatures_before_revocation_remain_valid}`. Scope
`vault:admin`. Revocation must make the key unusable for new signatures while
retaining every prior record: erasing what a creator signed is a different
operation and is not offered by this interface. Also needed: the counterpart
`POST {base}/vault/keys/{key_id}/recover`, so a creator regains a verified
identity without losing history. Fail: 409 already revoked, 403 not the key owner,
423 pending recovery hold.

**Not claimed.** Portable revocation. See "What a third party cannot check".

### 9. Signing history

**Today.** `signing_history(key_id)` replays `history.jsonl` and returns every
`sign` event for the key, before and after revocation. It uses a dedicated
append-only writer, deliberately not the daemon's rotating JSONL helper, which
drops oversize records and would silently delete evidence a verifier depends on.

**Remote must expose.** `GET {base}/vault/keys/{key_id}/history?cursor=&limit=` →
200 `{entries:[{history_entry_id, signed_at, payload_sha256, signature_b64,
content_sha256?, registry_id?}], next_cursor, total, log_inclusion_proof?}`. Scope
`vault:read`. History must remain readable after revocation, must be append-only,
and should carry a transparency-log inclusion proof so a third party can confirm
nothing was removed. Fail: 404 unknown key, 403 not the key owner.

**Not claimed.** Tamper-evidence. A local append-only file is a convention, not a
proof. The inclusion proof is what would make it one, and we do not produce it.

## The four verification states

The published vocabulary is exactly `verified`, `registered-but-changed`,
`mark-found-claim-not-trusted`, `nothing-found`. We use the same four values as
snake_case identifiers, defined once in `VerificationState`.

Two independent graders emit them from different evidence. This distinction is
load-bearing:

- **C2PA grader**: `daemon/c2pa_engine/verifier.py::verify_asset`. Grades the
  c2pa-rs validation report for the signed asset. **This is the live path**,
  running in `daemon/manifest_builder/generator.py` at export and in
  `daemon/verify.py` at re-verification.
- **Provider grader**: `LocalReferenceProvider.verify`. Grades a registry and mark
  lookup. Exercised by the provider API and its tests; no call site in the export
  pipeline today.

| state | C2PA grader means | provider grader means |
|---|---|---|
| `verified` | Signing credential chained to a supplied trust anchor and the hard binding is intact | A registry record matched, its receipt signature and chain validate, the key is not revoked, and the asset digest equals the registered one |
| `registered_but_changed` | Credential trusted, hard binding broken (`assertion.dataHash.mismatch`, `bmffHash`, or `boxesHash`) | Record trusted, but the asset's digest differs from the registered `content_sha256` |
| `mark_found_claim_not_trusted` | Provenance present but the signer was not established: no anchors supplied, credential did not chain, or a non-hash failure code | Provenance presented with no backing record; or a record whose chain, revocation state, receipt signature, or claim signature failed |
| `nothing_found` | No embedded manifest and no sidecar | No mark, no registry record, no manifest |

**Ordering rule, in both graders.** An untrusted signer outranks a broken hard
binding. `registered_but_changed` asserts the claim *was* trusted and only the
bytes moved. Anyone can self-sign a file and flip a byte; grading that as a
registration this system recognises is strictly stronger than the truth. Pinned by
`tests/test_trust_regressions.py::C2paClassificationTests`.

**`nothing_found` is not a synthetic-origin signal.** It records the absence of
provenance data, not the presence of a generation signal. The interface carries
`NOTHING_FOUND_NORMATIVE_NOTE` in every provider verify result, and
`tests/test_provenance_provider.py::test_nothing_found_is_not_an_assertion_of_synthetic_origin`
asserts the reason string never contains the word "synthetic". A caller that
renders `nothing_found` as an AI-detection verdict is misusing the interface.

**An unreachable registry is an error, not a verdict.** The remote contract
requires 503 specifically so it can never collapse into `nothing_found`.

## Proof levels

Every result carries `apw:proof_level`, one of `directly_observed`, `inferred`,
`user_declared`, `externally_verified`, `unknown_unobserved`. As implemented:

| operation | level | why |
|---|---|---|
| `identity()` | `user_declared` | The key exists and this machine chose the name on it |
| leaf issuance, `sign_claim`, `revoke`, `register` | `directly_observed` | This process performed the act and wrote the record |
| `embed_mark`, `recover_mark` | `inferred` | Descriptor resemblance, not identity |
| `verify` matched by `content_sha256` or `manifest_sha256` | `directly_observed` | Exact digest match |
| `verify` matched by `mark_id` | `inferred` | Similarity match; a verdict reached through the mark alone can never be stronger |
| `verify` with no match | `unknown_unobserved` | Nothing was established |

A remote service's `identity_proof_level` maps straight onto this field and we
never upgrade it locally. `externally_verified` is reachable only when a service
returns it.

## Key possession is not verified identity

The local reference generates a P-256 root CA (`CA:TRUE`, `path_length=1`,
critical `keyCertSign`) and issues itself a leaf (`CA:FALSE`, critical
`digitalSignature`, critical `emailProtection` EKU, `authorityKeyIdentifier`
matching the root's SKI). `validate_certificate_chain` enforces that profile and
rejects a self-signed leaf outright, because the C2PA verifier does.

That chain proves one thing: whoever produced the signature held the private key.
It does not establish that the subject common name belongs to the creator, that
any authority reviewed the creator, or that the certificate appears on a
recognised C2PA trust list. `identity()` says so in a `limits` field on every
call, and the generated claim records `"signer_identity": "not_established"` and
`"trust_anchor_scope": "self_issued_local_root_only"`.

This is exactly the gap a real vault closes. Everything else is already shaped for
it.

### What a third party cannot check

`_claim_trust` reads revocation state from this machine's custody store
(`keys/<key_id>/meta.json`) and validates against this machine's root. A third
party holding the asset and the receipt can verify the receipt signature offline
given the root certificate, but **cannot** learn whether the key was later
revoked. Portable revocation needs a published CRL, OCSP, or the remote vault's
`identity` and `revoke` endpoints. We do not have it and do not claim it.

## The soft binding is a deterministic marker, not a watermark

`embed_mark` returns `asset_modified: False`. Not one audio sample is altered. The
"mark" is an HMAC over a coarse feature descriptor, recorded in a local side
index.

Mechanism, exactly: up to 24 contiguous windows of 0.25 s taken from the start of
the file, three features per window (RMS in dB, ZCR in log10, crest as a linear
ratio), quantised into 64 / 48 / 24 buckets respectively. RMS and ZCR are bucketed
in the log domain because linear bucketing on [0,1] crowded unrelated tones into
identical descriptors and the binding matched anything; crest is bucketed linearly
against a ceiling of 12.0. A full-length descriptor is 72 integers; a shorter
asset yields fewer (a 3-second file gives 12 windows, 36 dimensions). Recovery
requires at least 90 % of dimensions to agree within ±1 bucket, using
per-dimension agreement rather than an averaged distance so a large disagreement
in one feature cannot hide behind agreement in the others.

Measured limits, stated as measurements and not as claims:

- **Only the first 6 seconds of an asset are described.** 24 windows × 0.25 s,
  taken contiguously from the file start; `extract_feature_sequence` truncates
  there. A four-minute master is characterised by its intro, and two masters that
  share an intro will collide regardless of what follows.
- **Inaudibility: not applicable.** No samples change, so there is nothing to mask
  and no PEAQ/ODG figure to report.
- **Survival through lossy transcode, resampling, or re-recording: unmeasured.**
  We have not built the codec/bitrate/operation matrix. That is what
  `measured_survival` in the remote contract is for.
- **The descriptor is not a bit-exact hash.** Flipping the low bit of one 16-bit
  sample, a sub-quantisation perturbation, still recovers the mark, which is what
  lets the provider grader report `registered_but_changed` rather than losing the
  asset entirely. That is a statement about the descriptor's coarseness, not a
  survival result.
- **Unrelated material does not match.** A 440 Hz tone does not recover a 220 Hz
  registration. Both this and the previous point are asserted in
  `tests/test_provenance_provider.py::test_four_state_mapping`. Two observed
  points, not a survival matrix.
- **Collisions are expected.** Two takes near-identical in loudness,
  zero-crossing rate, and crest quantise alike. This is why a mark-only match is
  reported at `inferred` and never higher.
- **The mark does not travel with the asset.** Recovery requires this machine's
  `marks.jsonl`. Move the file to another machine and recovery returns `None`.
- **Uncompressed PCM WAV and AIFF only.** Nothing else can be described.

A production soft binding sits behind exactly this interface and changes nothing
above it.

## Gaps in the remote contract

Named as gaps, not written as contract, because the stub does not specify them:

- No enrolment or identity-proofing flow. `issue_signing_material` returns 409 "no
  valid cert (needs enrolment)"; what enrolment *is* remains the service's call.
- No token endpoint shape. The auth model names client credentials and scopes but
  not the issuer, token lifetime, or rotation.
- No pagination contract for registry lookups, only for signing history.
- No schema version negotiation. The manifest shape we submit is
  `docs/manifest.schema.json`; nothing tells the service which version it is
  receiving.

## What has actually been run

`c2pa-python 0.90.15`, `.venv/bin/python`, 31 August 2026.

- `tests/test_provenance_provider.py`: 6 passed. Covers the chain profile (a
  self-signed leaf and a foreign-root leaf both rejected), revocation blocking
  signing while retaining history, the four-state mapping end to end, the
  `nothing_found` normative note, and that issued material is accepted by the real
  c2pa-rs signer and reads back as `Trusted`.
- `tests/test_trust_regressions.py`: a hand-appended `registry.jsonl` record
  carrying a genuine chain and a fabricated `content_sha256` is graded
  `mark_found_claim_not_trusted` on the receipt signature, not accepted.
- External confirmation of the signed asset is recorded in `docs/VALIDATION.md`:
  `c2patool 0.26.68`, run outside this pipeline, reads the embedded WAV claim as
  `Trusted` with zero failures against the issuing root and as
  `signingCredential.untrusted` without it. c2patool shares the c2pa-rs core with
  the binding that wrote the claim, so no second implementation has agreed yet,
  and the AIFF sidecar path has not been externally validated.
