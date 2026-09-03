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

## Deliberately deferred

- The capture application and SDK currently have separate local provider/trust-store formats.
- The capture installer ships the frozen Python daemon; the Rust capture engine under `rust/` is
  built and tested separately.
- The TypeScript/WASM surface verifies caller-supplied records but cannot sign or publish.
- No production identity authority or portable revocation service is configured. Remote key
  custody and authenticated registry publication are implemented but still require deployment
  credentials and a trust anchor before a release can name an identity.
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
