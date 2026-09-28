# Capture and SDK integration

## Position

This monorepo owns both sides of the audio-provenance workflow:

```text
Ableton creation session
  -> routed observations and coverage counters
  -> exported audio + signed C2PA claim + evidence bundle
  -> SDK admission and registration
  -> hard-binding or resilient recovery
  -> trust evaluation and four-state verification
```

The capture application and the SDK are complementary runtimes joined by one explicit adapter.
The adapter is outside the plug-in and audio thread: the daemon completes and signs capture
evidence first, then invokes the public Rust SDK through the same CLI used by direct callers.

## Implemented boundary

- `daemon/` produces `audio-provenance-manifest-v0` records, evidence bundles, registration
  handoffs, and embedded-WAV or detached-AIFF C2PA claims.
- `sdk/crates/audio-provenance-core` reproduces the Python canonical signing bytes.
- `sdk/crates/audio-provenance-manifest` admits real manifests produced by the capture daemon and
  enforces their proof-level and association invariants.
- The capture application's `LocalReferenceProvider` implements identity, signing material,
  signing, marker lookup, registration, verification, revocation, and history locally.
- The SDK implements native signing, marking, registry publication, trust-chain evaluation,
  recovery, CLI verification, and a verify-only TypeScript/WASM facade.
- `audio_provenance_sdk::adapt_capture_export` admits the signed capture manifest and exact nested
  handoff, preserves every proof level, streams the evidence-bundle digest, signs and embeds or
  sidecars the SDK record, optionally publishes it, and verifies the result through the public SDK.
- `daemon/sdk_adapter.py` contains no independent mapper or signer. It invokes
  `audio-provenance capture-adapt`, so Python orchestration and a direct Rust CLI invocation share
  canonicalization, signing, record identifiers, embedding, and verification.
- Adapter retries are idempotent for the canonical record and record identifier. Locator salt is
  derived from the configured signer's public key and immutable input digests. Both the local
  registry and authenticated HTTP `PUT /v0/record/{record_id}` protocol tolerate republishing the
  same record after a crash.

## Landed since the last deferral

- Revocation reaches the TypeScript/WASM verifier. `options.trustStore` accepts the chained
  `audio-provenance-trust-store-v1` document, whose Ed25519-signed revocation lists are the same
  ones `audio-provenance trust revoke` writes and the Rust verifier already consumed. A revoked key
  fails closed (`untrusted`, `trust_anchor_rejected` starting `signer_revoked`, no identity).
- Every result carries `revocationStatus`: `not_applicable` (no identity), `checked_not_revoked`
  (a list signed by the vouching anchor was consulted), `revoked`, or `revocation_unchecked`
  (an identity resolved but no list from its anchor was available, with a `revocation_unchecked`
  warning finding). The last is never reported as ok. Flat v0 stores count as unchecked unless they
  declare a `revoked` array, which is unsigned.
- WASM has no clock it may trust: a v1 store is judged at `trustEvaluatedAt`, defaulting to the
  JavaScript clock. Pin it for reproducible results.
- A pluggable signer for TypeScript/WASM: `sign(audio, signer)` with
  `signer = { publicKeyHex, sign(digest32) -> signature64 }` (`webCryptoSigner` adapts a WebCrypto
  Ed25519 key handle). The package prepares the record, hands the signer a SHA-256 digest
  (`Ed25519-SHA256`, the algorithm the remote key-custody Worker returns), verifies the returned
  signature under the declared public key, and re-admits the sealed record before returning it. No
  API takes a private key. The record claims `self_generated_demo_key_integrity`, the weakest
  existing trust scope, because the package cannot observe where the caller's key lives; identity
  stays `not_established`.

## Deliberately deferred

Needs an outside decision or infrastructure; nothing below is stubbed:

- Production identity authority. Needs a decision on who operates the trust anchor and the
  issuance governance behind it. Until one is configured, identity stays `not_established` and only
  a caller-configured anchor can attach a name.
- Revocation distribution and freshness. Lists are caller-supplied; no service publishes them and
  nothing fetches them. Lists carry `issued_at` but no `next_update`, so a stale list is
  indistinguishable from a current one. Needs a publication endpoint and a freshness policy.
- Deploying the remote key-custody Worker (`sdk/workers/hsm-signer`). Needs a Cloudflare account,
  `SERVICE_TOKEN` and `SIGNING_KEY_PKCS8` secrets, and, for a hardware non-exportability claim, a
  managed HSM/KMS instead of the Worker's secret binding.
- Authenticated registry publication. The client exists; it needs deployment credentials and a
  trust anchor before a release names an identity. The TypeScript/WASM surface still cannot
  publish or embed marks; it signs only through the two-call signer above.
- CMS verification of a time-stamp token against a TSA certificate chain. Needs a choice of TSA and
  root set and a decision on an ASN.1/CMS parser dependency.
- The capture application and SDK still use separate local provider/trust-store formats, and the
  capture installer ships the frozen Python daemon while the Rust capture engine under `rust/` is
  built and tested separately. Both live outside `sdk/` and need a cross-component decision.
- Ableton-only compatibility checks remain separate from the automated headless lifecycle/soak
  gate. The classical watermark baseline is versioned and currently records target failures.

## Trust boundary

- Routed buffers and export hashes are directly observed; stem-to-export association remains
  inferred.
- A daemon acknowledgement is a local operational receipt, not DAW trust or identity proof.
- Self-issued C2PA and Ed25519 credentials prove key possession and integrity, not creator identity.
- The watermark is a recovery locator, not an authenticator or tamper-resistance mechanism.
- A missing mark or record is absence of provenance evidence, never evidence of synthetic origin.
- Only an explicitly configured trust anchor can attach a creator or studio name to a verified
  result.

## Development round trip

Run one command from the repository root. It builds and locates this checkout's SDK CLI when no
explicit `--sdk-cli` override is supplied:

```sh
./.venv/bin/python scripts/synthetic_rehearsal.py --sdk-adapter
```

For a live daemon, add `--sdk-adapter` and optionally `--sdk-cli`,
`--sdk-development-key`, and `--sdk-registry`. The default development key is created with mode
`0600` and reused only inside that run so a crash retry yields the same record. Fixtures use fixed
test keys. Identity is always `not_established`, and the signed SDK record carries prominent
`development_only` metadata.

## Production remote custody

`CaptureAdapterOptions::production(CloudflareSigner)` uses the signing trait instead of loading a
raw key from disk. It omits the `development_only` claim and omits the receipt's development
`identity` override. `HttpRegistryBackend::new_authenticated` implements authenticated,
idempotent network publication and can use a bearer token or Cloudflare Access service-token
headers.

The production path does **not** manufacture identity: a remote signature proves possession of the
pinned public key, and only a verifier trust anchor can bind that key to a creator or studio. The
Worker and deployment instructions are in `sdk/workers/hsm-signer/README.md`.

The completed adapter:

1. accepts the capture handoff and evidence-bundle digest;
2. admits the signed `audio-provenance-manifest-v0` record and requires the separately supplied
   handoff to equal the signed nested value;
3. maps it into the SDK record without raising any proof level;
4. signs, optionally publishes, and embeds an `aprv` slot into a copy or writes a sidecar;
5. verifies the exported asset through the same public SDK surface used by third parties.

The adapter result and record identifier are written atomically to a receipt beside the signed
capture manifest and linked from daemon status. They are not inserted into the capture manifest
after signing: that would invalidate its signature, and inserting them into the evidence bundle
whose digest the SDK record commits to would introduce a circular dependency.
