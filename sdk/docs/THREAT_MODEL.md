# Threat model

Adversarial audit of the Audio Provenance verification path, 2026-08-31. The attacker has read all of this
source, can supply arbitrary audio, arbitrary sidecars, arbitrary embedded container chunks, and can
operate or compromise a registry. They cannot read the verifier's private keys and cannot edit the
verifier's trust store.

Every finding and every inventory row carries a reachability tag:

- **PRODUCTION** — a non-test call path reaches it, with the call site cited.
- **TEST-ONLY** — the only callers are under `#[cfg(test)]`, `tests/` or `benches/`.
- **DORMANT** — no caller at all.

`file:line` references are to files opened during the audit. Line numbers for
`crates/apw-trace/src/ladder.rs` are as of the fix in F-1 below.

## Scope

Read and audited: `audio-provenance-core`, `audio-provenance-manifest`, `audio-provenance-registry`, `apw_trace`,
`audio-provenance-trust`, `audio-provenance-sdk`, `audio-provenance-cli`, `audio-provenance-wasm` (options only), `audio-provenance-c2pa`,
`apw_watermark` (payload, CRC, keystream, params), `audio-provenance-audio` (wav, iff, buffer, decode limits).

**Not read, and therefore not reported on:** `crates/apw-watermark-neural`, `training/`, `tools/capture-rig`
(owned by the concurrent v2 effort). The assignment's ONNX-model and ZIP-bundle parser surfaces live
there. No ONNX or ZIP parsing exists in any crate this audit read; the only archive-shaped input in
scope is the JUMBF box tree, covered under section 3. `audio-provenance-bench` DSP internals and
`apw-trace/src/fingerprint/{landmark,score,reference}.rs` were read only where the verification path
enters them.

---

## 1. Cryptographic inventory

| Primitive | Crate + version | Where | Reachability |
|---|---|---|---|
| Ed25519 (`verify_strict`) manifest signature | `ed25519-dalek` 3.0.0 (`curve25519-dalek` 5.0.0) | `audio-provenance-core/src/signing.rs:265-301` | **PRODUCTION** — `audio-provenance-manifest/src/unverified.rs:124`, reached from `apw-trace/src/admission.rs:57` on every rung |
| Ed25519 domain-separated signature | `ed25519-dalek` 3.0.0 | `audio-provenance-core/src/signing.rs:94-98, 305-318` | **PRODUCTION** — trust anchors/records/revocations, `audio-provenance-trust/src/document.rs:200,354,514` |
| SHA-256 | `sha2` 0.11.0 | `audio-provenance-core/src/hashing.rs:31-37` | **PRODUCTION** — content hash, decoded-audio hash, record id, signer id, locator |
| SHA-256 hash chain (`window_hash`) | `sha2` 0.11.0 | `audio-provenance-core/src/hashing.rs:12-29` | **TEST-ONLY in this workspace.** `grep -rn window_hash crates/*/src crates/*/tests` returns only the definition, the `audio-provenance-core/src/lib.rs:18` re-export and `audio-provenance-core/tests/poc_interop.rs:139-158`. It is exported for POC observer interop; no verification path calls it. Note that `window_hash("", s)` and `window_hash("genesis", s)` are equal by design (`hashing.rs:14-18`), so a chain whose previous hash is the literal string `genesis` is indistinguishable from a genesis window. Not exploitable here because nothing in this workspace consumes it |
| HKDF-SHA256 (Watermark schedule) | `hkdf` 0.13.0 + `hmac` 0.13.0 | `apw-watermark/src/keystream.rs:38-40` | **PRODUCTION** — `Watermark::public()` at `apw-trace/src/lib.rs:234,276` |
| ChaCha20 keystream (dither/preamble/pilot) | `chacha20` 0.10.2 | `apw-watermark/src/keystream.rs:47` | **PRODUCTION**, same path |
| CRC-32C (Castagnoli, reflected 0x82F63B78) | hand-rolled, `apw-watermark/src/crc32c.rs` | payload acceptance gate, `apw-watermark/src/payload.rs:139-152` | **PRODUCTION** |
| CSPRNG | `getrandom` 0.4.3 | `audio-provenance-cli/src/cmd/keygen.rs:29`, `cmd/sign.rs:189`, `cmd/embed.rs:187`, `audio-provenance-sdk/src/producer.rs:334` | **PRODUCTION** — all key and salt material |
| PCG-XSH-RR 64/32 | hand-rolled, `audio-provenance-bench/src/dsp/rng.rs` | bench reproducibility only | **PRODUCTION in the bench binary, never for key material** |
| TLS | `rustls` 0.23.43 via `ureq` 3.4.0 | `audio-provenance-registry/src/http.rs:163-171` | **PRODUCTION** when an HTTP registry is configured; `http` is no longer a default feature |
| COSE / ES256 / PS256 (C2PA) | — | not implemented anywhere | **DORMANT** — `apw-trace/src/ladder.rs:404-407` emits `c2pa_cose_verification_unsupported` |
| Post-quantum | — | none | absent, correctly |

There is **one** signature algorithm. `SignatureAlgorithm` is a single-variant enum
(`audio-provenance-core/src/signing.rs:19-23`), `audio-provenance-trust`'s `ALGORITHM` constant is the literal
`"ed25519"` and every document parse refuses anything else at
`audio-provenance-trust/src/document.rs:548-557`. There is no algorithm field an attacker can move, and no
`alg`-driven dispatch. Algorithm confusion is structurally unavailable.

### Asymmetry sweep: producer vs consumer

| Pair | Producer | Consumer | Agree? |
|---|---|---|---|
| Manifest signature | `SigningKey::sign_manifest`, `signing.rs:102-123` — signs bare `canonical_json` | `verify_manifest_signature`, `signing.rs:265-301` — same bytes, same canonicalizer | Yes |
| Trust documents | `sign_domain_separated`, `signing.rs:94` — `TYPE \|\| 0x00 \|\| canonical_json` | `verify_domain_separated`, `signing.rs:305` | Yes |
| Cross-protocol replay | manifest preimage always starts `{`; trust preimage always starts with a type string; no NUL may appear in a domain (`signing.rs:323`) | — | **Disjoint input spaces, verified** |
| Key encoding | raw 32 bytes, lowercase hex everywhere (`public_key_hex`, `subject_public_key_hex`, `issuer_public_key_hex`) | `decode_fixed::<32>` / `Reader::hex::<32>` — exact length, lowercase-only | Yes. No SEC1/DER/JWK/Multikey anywhere |
| Signature encoding | fixed 64-byte `r\|\|s`, hex | `decode_fixed::<64>` / `Reader::hex::<64>` | Yes. No DER path exists, so the "64 bytes could be either" trap does not arise |
| Locator | `derive_locator(pubkey, salt)`, `locator.rs:99` | `locator_from_signed_manifest`, `locator.rs:119` — one implementation shared by writer and re-deriver | Yes |
| Record `mark_id` | `RegistryRecord::from_signed_manifest`, `record.rs:78-85` — derived from the manifest's own key and salt | `LocalRegistryBackend::read_entry` `local/mod.rs:150`, `HttpRegistryBackend::lookup_by_mark` `http.rs:400` | Yes — a backend cannot assert a mark id |
| Record `content_sha256` | envelope field, caller-supplied (`record.rs:54-59`) | not cross-checked against `hard_binding.content_sha256` | **No** — see F-7 |
| Watermark payload → record | `audio-provenance sign` allocates the locator, then `audio-provenance embed` writes it | rung 5 re-derives (`ladder.rs:733-771`); route 3(c) re-derives (`ladder.rs:1207`); the no-hard-binding soft arm did **not** | **No, until F-1's fix** |

There is no hardware-backed signer, no Secure Enclave, TPM, HSM or KMS path, and no `cfg!(test)` or
emulator guard anywhere on the signing path. `SigningKey` has exactly one constructor from bytes
(`signing.rs:65`) and one from a file (`signing.rs:356`). The producer/consumer implementation is the
same code on every platform, including the wasm build.

### Key derivation tree

```
operator entropy  ── getrandom::fill (32 B) ──▶ Ed25519 seed  [keygen.rs:28-34]
                                                    │
                                                    ├─▶ signing key   (ed25519-dalek, zeroized seed at keygen.rs:37)
                                                    └─▶ public key P (32 B)
                                                            │
                                                            ├─▶ signer_id = sha256(P)[0..8]  hex, 16 chars   [signing.rs:46-50]
                                                            │        └─ used as a trust-store KEY in the v0 store  ── F-2
                                                            │
                                                            └─▶ locator = sha256("audio-provenance-locator-v1" || 0x00 || P || salt)[0..6]
                                                                     ▲                                        [locator.rs:99-111]
                                          getrandom::fill (16 B) ────┘  locator_salt, signed into the manifest


Watermark profile key  (namespace 0: the literal b"audio-provenance/apw-watermark/public/v1", params.rs:122)
        │
        └─ HKDF-SHA256, salt = NONE, info = "apw-watermark-lepqim-v1" || namespace(1 B) || epoch(8 B, BE)
                 │                                                        [keystream.rs:32-40]
                 └─▶ 44 bytes ─┬─ 32 B ChaCha20 key
                               └─ 12 B ChaCha20 nonce
                                        │
                                        └─▶ keystream ─┬─ 1664 B  dither lattice   (416 slots x 4 B)
                                                       ├─   32 B  preamble PN
                                                       └─   96 B  pilot polarities
```

Notes on the KDF, stated exactly:

- The HKDF salt is `None` (`Hkdf::<Sha256>::new(None, profile_key)`), i.e. the all-zero salt. That is
  acceptable here because the IKM is a fixed constant and the whole schedule is public by design for
  namespace 0.
- The ChaCha20 **nonce is derived from the same HKDF output as the key**, so it is a deterministic
  function of `(profile_key, namespace, epoch)`. There is no per-message randomness and no counter.
  This is not a nonce-reuse defect because the construction is a PRG expansion, not an encryption of
  distinct plaintexts: the same `(key, nonce)` pair is *required* to reproduce the same schedule at
  detect time. There is no birthday bound to state because no message is ever encrypted under it.
- The epoch is `BLOCKS_PER_EPOCH = 256` blocks, so the schedule rotates every 256 blocks
  (`params.rs:38`).

### Zeroization

| Secret | Zeroized? | Where |
|---|---|---|
| 32-byte Ed25519 seed in `keygen` | Yes, on both success and entropy failure | `keygen.rs:30,37` |
| Seed buffer in `SigningKey::from_raw_bytes` | Yes | `signing.rs:71` |
| Key file bytes in `load_signing_key` | Yes | `signing.rs:358-360` |
| `ed25519_dalek::SigningKey` itself | Relies on the dependency's `zeroize` feature, which is enabled at `Cargo.toml` workspace deps. `audio_provenance_core::SigningKey` adds no `Drop` of its own | `signing.rs:52-54` |
| `SigningKey` `Debug` | Redacted — prints only `signer_id` | `signing.rs:56-62` |

`SigningKey` is not `Clone` and not `Copy`, so the usual clone-defeats-zeroize failure is unavailable.
`getrandom::fill` writes into a fixed `[u8; 32]`, not a `Vec`, so no realloc can leave a copy behind.

**No key material reaches a log or an error.** Every error variant carrying key-adjacent data carries
a *public* key or a length: `KeyError::PrivateKeyLength { found }` is a length only
(`audio-provenance-core/src/error.rs:38`); `TrustError::Inconsistent` carries `hex::encode` of *public* keys
(`document.rs:539-543`). `CliError` was grepped for secret-bearing variants and carries none.

### Constant-time comparison

There is **no comparison of secret material anywhere on the verification path**, so the absence of
constant-time helpers is correct rather than an omission. Every equality this system performs is over
public data: digest hex strings (`binding.rs:86-92`), locators, public keys, signer ids. Signature
verification itself is `ed25519_dalek::verify_strict`, whose internals are the dependency's
responsibility (`subtle` 2.6.1 is in the tree). `hex::decode_to_slice` and `hex::decode` are not
constant time and do not need to be.

---

## 2. Findings

### F-1 — Mark transplant verifies against a record that declares no hard binding — HIGH — PRODUCTION — **FIXED IN THIS AUDIT**

**What.** `evaluate_binding` fell through to the Watermark soft-binding arm whenever
`manifest.hard_binding()` was `None`, and that arm accepted **any** CRC-valid payload recovered from
the audio without ever checking that the payload's locator names the record being evaluated.
`corroborate`, the route-3(c) sibling twenty lines below, does make exactly that check
(`ladder.rs:1207`). The two arms disagreed.

**Reachability.** PRODUCTION.

- The class is constructed at exactly one non-test site. `grep -rn "SoftMark" crates/*/src` returns
  `crates/apw-trace/src/ladder.rs:1098` and `:1105` as the only constructors of
  `BindingEvaluation::SoftMark` / `SoftMarkUnpriced`; every other hit is the enum definition, the
  class projection in `result.rs`, or the status mapping.
- `evaluate_binding` is called from `apw-trace/src/lib.rs:411`, inside `run`, which is the body of
  both `verify` (`lib.rs:199`) and `verify_bytes` (`lib.rs:207`) — the CLI's `verify` command, the
  SDK facade and the wasm surface all land there.
- The fall-through requires `hard_binding()` to be `None`. `HardBinding::evaluate_content` compares
  `Some(&self.content_sha256)` unconditionally (`binding.rs:74-76`), so it can never return
  `Uncoverable`; the `Uncoverable => {}` arm at `ladder.rs:1066` is dead. `None` is produced only by
  `ManifestSchema::ApwV0` with no `export.sha256`
  (`unverified.rs:135-141`, `read_apw_binding` at `:213-225`), and `export` is **not** in
  `validate_apw_invariants`' required-field list (`invariants.rs:23-35`, and `validate_export`
  returns early on absence at `invariants.rs:169-171`).
- The wasm build has no sidecar rung, but it reaches the same arm through the embedded RIFF `aprv`
  slot (`container.rs:14,59-80`), which STATUS.md records as a live route.

**The precondition is met by a conforming producer, not only by a hostile one.** The comment at
`audio-provenance-manifest/src/invariants.rs:37-40` asserts that "The JSON Schema additionally demands
`export`". **That comment is wrong**, and it is the sentence that would talk a reviewer out of this
finding. `/Volumes/A/audio-provenance/docs/manifest.schema.json:6-24` lists seventeen
required fields and `export` is not among them; `daemon/schema.py:53-55` reads it with
`data.get("export")` and validates it only if present; and the POC's own builder emits it
conditionally — `daemon/manifest_builder/builder.py:204`, `if self.export is not None:`. A POC session
that produced no rendered file therefore signs exactly the shape this finding needs. The comment
should be corrected; it is in a crate the concurrent run owns, so it is reported rather than edited.

**The attack.** Namespace 0's Watermark profile key is the published literal
`b"audio-provenance/apw-watermark/public/v1"` (`apw-watermark/src/params.rs:122`), and `Watermark::public()` is what
`apw_trace` uses when the caller supplies none (`apw-trace/src/lib.rs:234,276`). Anyone can therefore
mint a strong, CRC-valid mark carrying **any** 48-bit locator; locators are not secret, they are URL
path segments (`http.rs:367`). An attacker takes a published `audio-provenance-manifest-v0` document
signed by an anchored studio that carries no `export.sha256`, drops it beside arbitrary audio they
marked themselves as `arbitrary.wav.audio-provenance.json`, and rung 2 admits it (`ladder.rs:459`). The
signature is genuine, the invariants pass, the mark is strong and covers the file. With a null-test
report supplied, the class is `SoftMarkStrongAtOrAboveThreshold` and the mapping returns
`Verified / soft_binding_accepted / externally_verified` (`status.rs:311-315`), naming the studio.
Without a null-test report it is `SoftMarkFalsePositiveRateUnknown` → `untrusted`, so `--null-test`
is what turns the hole from loud to silent.

**Fix applied** at `crates/apw-trace/src/ladder.rs:1081-1118`: the soft arm now requires
`manifest.mark_locator() == Some(payload.locator_bytes())`, the same term `corroborate` applies. A
mark that names another record raises a `recovered_mark_names_another_record` warning and the
evaluation falls through to the fingerprint arm or to `NoEvidence`, i.e. `changed`. `mark_locator()`
re-derives from the manifest's own signed `portable_signature.public_key_hex` and `locator_salt`
(`manifest.rs:257-259` → `locator.rs:119-128`), so it cannot be asserted by the attacker.

**Blast radius of the fix is zero for the Audio Provenance schema.** `ManifestSchema::AudioProvenanceV1` requires
`hard_binding` (`unverified.rs:143`, `read_audio_provenance_binding` at `:232-249`, and `hard_binding` is in
`AUDIO_PROVENANCE_REQUIRED_FIELDS` at `invariants.rs:661-668`), so no `audio-provenance sign` output ever reaches this
arm. An ApwV0 record with no `locator_salt` now cannot be soft-mark-verified at all, which is the
correct statement: such a record commits to no locator, so no mark in any audio is evidence about it.

**Demonstrated, not derived.** `crates/apw-trace/tests/soft_mark_locator.rs` is a new file that
drives both halves end to end through `verify` over a real 29 s WAV, a real Watermark embed and a real
`audio-provenance-manifest-v0` fixture with `export` removed, `locator_salt` added and the document
re-signed:

- `a_mark_that_names_this_record_substitutes_for_the_hard_binding_it_never_declared` — the arm is
  live and reaches `Verified / soft_binding_accepted / SoftWatermark / match 0.99`. This is what
  proves the surface was real rather than theoretical.
- `a_mark_that_names_another_record_is_not_evidence_about_this_one` — with the fix, `Changed /
  no_binding_evidence / match 0.0` plus the `recovered_mark_names_another_record` finding.

**Negative control, run.** With the locator term temporarily replaced by `if true` at
`ladder.rs:1089`, the second test fails with the exploit in the assertion output, verbatim:

```
test a_mark_that_names_another_record_is_not_evidence_about_this_one ... FAILED
VerifyResult { status: Verified, reason: "soft_binding_accepted",
  identity: Some("Signal Room Studios"), identity_proof_level: ExternallyVerified,
  identity_authority: Some("studio-ca"), match: 0.99, match_basis: Watermark,
  binding: BindingReport { kind: SoftWatermark, match: 0.99, proof_level: DirectlyObserved,
    threshold: 0.72, false_positive_rate_at_match: Some(0.008108108108108109),
    detail: "Watermark, 3/3 blocks" },
  signature: Some(SignatureReport { ..., valid: true }) }
```

That is an anchored studio's name printed as `verified` over audio carrying a mark that names a
different record. `ladder.rs` was restored byte-identically from a backup afterwards and re-verified.

**Verified.** `cargo test -p apw_trace` → **31 passed, 0 failed, 1 ignored**.
`cargo clippy -p apw_trace --all-targets --all-features -- -D warnings` clean.
`cargo fmt --check -p apw_trace` clean. The wide run
`cargo test -p audio-provenance-core -p audio-provenance-audio -p audio-provenance-registry -p audio-provenance-bench -p apw_watermark
-p audio-provenance-manifest -p apw_trace -p audio-provenance-cli -p audio-provenance-trust -p audio-provenance-c2pa` exits 0 with
**183 passed, 0 failed, 6 ignored**. **No existing test was changed**; one new file was added.

`-p audio-provenance-sdk` is absent from that list, and the reason is not this audit's change. At 17:17 the
concurrent v1-completion run added `crates/audio-provenance-sdk/tests/facade.rs:453`, which reads
`result.record_id`, a field `VerifyResult` does not yet have
(`crates/apw-trace/src/result.rs`, unmodified since 11:55): `error[E0609]: no field record_id on type
VerifyResult`. That test target has not compiled since. An earlier run of the same list *including*
`-p audio-provenance-sdk`, taken before that edit, exited 0 with the fix in place.

### F-2 — The v0 trust store binds a name to a 64-bit truncated key digest — HIGH — PRODUCTION — NOT FIXED

**What.** `apw_trace::FileTrustStore` keys its anchors and its revocation list on `signer_id`, which is
`sha256(public_key)` truncated to 16 hex characters — 64 bits (`signing.rs:46-50`,
`FileTrustStore::resolve` at `trust.rs:148-161`, `AnchorEntry` at `trust.rs:76-81`).

`audio-provenance-core` states the opposite requirement in its own doc comment, verbatim
(`signing.rs:239-241`):

> IMPORTANT: a trust store MUST bind its records to these 32 bytes, not to `Self::signer_id`. The
> signer id is a 64-bit truncation of the key's digest, so binding a name to it would make
> impersonation a 2^64 second-preimage search instead of a forgery.

**Reachability.** PRODUCTION, on three surfaces, in increasing severity:

- `crates/audio-provenance-sdk/src/verify.rs:84-85` — `with_trust_store_path` **unconditionally** constructs
  `FileTrustStore::load(path)`. There is no format branch and no sibling constructor;
  `audio-provenance-sdk/Cargo.toml` does not depend on `audio-provenance-trust` at all, so the chained v1 store is
  unreachable from the SDK except by a caller who builds `AnchoredTrustStore` themselves and passes
  `with_trust_store(Box<dyn TrustStore>)`. Worse than "the weak store is the default": it is the only
  one that loads. `FileTrustStore::from_json` compares `parsed.format` against
  `audio-provenance-trust-store-v0` and **errors** on anything else (`trust.rs:97-105`), so an operator who
  follows the CLI's own guidance and issues a `audio-provenance-trust-store-v1` store gets a hard error from
  the SDK and from the npm package.
- `crates/audio-provenance-wasm/src/options.rs:75` — `FileTrustStore::from_json` is the **only** trust-store
  path in the published `@writerslogic/audio-provenance-sdk` package. Its doc comment at `:27` names
  `audio-provenance-trust-store-v0` explicitly.
- `crates/audio-provenance-cli/src/context.rs:137-138` — reached only when the file declares
  `"format": "audio-provenance-trust-store-v0"`; a directory with `anchors/` or a v1 document takes the
  chained path at `:94` / `:140`.

**Cost of the attack, stated honestly.** The attacker needs a second preimage on a specific 64-bit
digest where each candidate costs one Ed25519 public-key derivation plus one SHA-256 — not a bare
hash. Incremental point addition makes a candidate cheap (order 1 µs on a CPU core, faster on GPU),
so 2^64 candidates is roughly 10^10 core-seconds: infeasible for an individual, within reach of a
well-funded adversary with a large GPU fleet over weeks. It is a genuine collapse of a 128-bit
security claim to a 64-bit one, on the one value that decides whether a name is printed. It also
weakens revocation identically (`trust.rs:152`).

**Remedy, precisely.** Add a required `public_key_hex` (32 bytes, lowercase hex) to
`apw_trace::trust::AnchorEntry`; key `FileTrustStore.anchors` and `.revoked` on `[u8; 32]`; have
`resolve` compare `proof.public_key_bytes()` (`signing.rs:242-244`) and treat a declared `signer_id`
that does not derive from the key as a load error. That is a deliberate break of the v0 format, whose
migration message is "re-issue as `audio-provenance-trust-store-v1`", which `audio-provenance trust` already writes
exclusively (`context.rs:87-88`).

**Not fixed here** because it changes an on-disk format and requires editing three test helpers
inside `crates/apw-trace/tests/recovery.rs:60-64`, `crates/apw-trace/tests/fingerprint.rs:340-344`
and `crates/audio-provenance-sdk/tests/facade.rs:52-65`, all of which construct the store from a bare
`signer_id` string and some of which do not have the public key in scope. Those crates are owned by
the concurrent v1-completion run.

### F-3 — An attacker-supplied manifest attributes `changed` to an anchored signer — MEDIUM — PRODUCTION — NOT FIXED (design decision, needs to be a stated one)

**What.** `(HardMismatch, Anchored)` maps to `Changed` at proof level `ExternallyVerified`
(`status.rs:288-292`), and `(NoBindingEvidence, Anchored)` does the same (`status.rs:340-344`).
`discloses_identity()` is true for `ExternallyVerified` (`status.rs:192-194`), so the name and the
authority are printed (`result.rs:547-553`, `render.rs:157-163`). The design intent is stated at
`status.rs:286-287`: "a known studio whose audio was altered is exactly what the row is for."

**Reachability.** PRODUCTION. Rung 2 reads sidecars beside the input file
(`ladder.rs:410-421`, `sidecar_paths` at `lib.rs:490-500`), and the sidecar travels with the audio,
so whoever distributes the file chooses it. The manifest need only be genuinely signed; published
records are exactly that. The behaviour is pinned by an existing test:
`crates/apw-trace/tests/recovery.rs:294-315`, `audio_that_moved_under_a_valid_signature_is_changed`,
asserts `result.identity.as_deref() == Some("Signal Room Studios")` on a `changed` verdict.

**Why it matters.** The row assumes the manifest arrived with the audio legitimately. An attacker
publishes unrelated audio plus a real signed manifest belonging to an anchored studio; every verifier
that has anchored that studio prints `changed` **and names them**. Nothing in the output distinguishes
"this studio's master was altered" from "someone pointed this studio's manifest at audio they never
touched," because there is no evidence that could.

**Recommendation, not applied.** Either drop identity disclosure on `HardMismatch` /
`NoBindingEvidence` when the association came from a rung the file asserted (`EmbeddedManifest`,
`SidecarManifest` — `RecoveryMethod` is already on the result), or keep it and make the rendered note
say that the association is asserted by the file and not by the signer. The status mapping cannot see
`RecoveryMethod` today, so this is a real interface change, not a one-line edit.

### F-4 — A single hostile or corrupted record halts the ladder — MEDIUM — PRODUCTION — NOT FIXED

**What.** `accept()` returns `true` on `Admission::Rejected`, which sets `outcome.rejected` and makes
`run()` stop descending (`ladder.rs:284-297`, `run` at `:213-234`). `Admission::Unparseable` correctly
returns `false` and the ladder continues (`ladder.rs:299-314`).

**Reachability.** PRODUCTION, two ways:

- **Container.** Insert a `aprv` RIFF chunk (`container.rs:14,59-80`) holding a JSON document that
  parses, declares `"schema": "audio-provenance-record-v1"` and carries a bad signature. Rung 1 classifies it
  `SignatureOutcome::Invalid` (`admission.rs:105`) and the verdict is
  `untrusted / signature_invalid`, terminal, on a file whose real embedded or registered manifest
  would have verified.
- **Registry.** A hostile registry answering rung 3 with any admissible record stops the ladder before
  rung 5 ever runs (`ladder.rs:221-224`).

**Severity.** This is a **downgrade in the safe direction** — the assignment's rule is that a
downgrade must never yield a *stronger* claim, and it does not: every rung feeds the same
`evaluate_binding`, so no lower rung has a higher ceiling than rung 1. What it costs is availability
and reputation: an attacker can turn a genuinely `verified` file into `untrusted` by appending a chunk.
Worth stating in the product's honest-limits section; not a soundness bug.

### F-5 — Watermark namespace 0 is a published key; the mark is recovery, not tamper resistance — INFO — PRODUCTION — correct as designed

`PUBLIC_PROFILE_KEY` is `b"audio-provenance/apw-watermark/public/v1"` (`params.rs:122`) and `Watermark::public()` is
the default in `apw_trace` (`lib.rs:234,276`), in the SDK (`audio-provenance-sdk/src/verify.rs:44`) and
therefore in the wasm build. `apw-watermark/src/lib.rs:71-74` already says so plainly: "an informed
adversary can estimate and subtract it. It is not tamper resistance and must not be described as
such."

The assignment asks whether a payload can be forged to pass CRC without the profile key. For
namespace 0 the question does not arise: the key is published, so **anyone can mint a strong,
CRC-valid mark carrying any 48-bit locator, and can strip an existing one**. Blind CRC-32C forgery
without the schedule is a different and much harder problem — the CRC gates the *decoded* message and
the schedule gates the decode — but it is not the relevant threat, because the schedule is free.

Consequence: the mark is an **index**, never an authenticator. Everything downstream is written that
way — the locator is re-derived from the record's own signed key and salt at `ladder.rs:746-750`, at
`:1207` and now at `:1089`. F-1 was the one place that assumption had been dropped.

One loose end, not exploitable: `DetectionOutcome::namespace_mismatch` (`apw-watermark/src/detect.rs:775`,
`:842`) is raised as a *warning only* at `ladder.rs:679-689` when the accepted payload's 4-bit
namespace field differs from the namespace the detector is keyed for. The declared field then becomes
part of the registry `MarkId` (`ladder.rs:694-698`), so the mismatch only redirects the lookup; the
schedule the block actually decoded under is the detector's. No verdict can be moved by it.

### F-6 — C2PA: no COSE verification, and stripping the store is not a downgrade — INFO — PRODUCTION — correct as designed

`record_c2pa_diagnostics` reads a C2PA store from a WAV or a sidecar, recomputes the hard binding, and
emits a `warning` finding (`ladder.rs:372-408`). It **never** produces a candidate and never touches
`derive_status`. `ClaimSignatureEvidence::NotEvaluated` classifies to `mark_found_claim_not_trusted`
(`audio-provenance-c2pa/src/validation.rs:159-164`), i.e. `untrusted`, so the crate cannot mint a `verified`
it did not earn. Stripping an embedded C2PA manifest therefore removes only a diagnostic; the rung
that answers is unchanged and its ceiling is unchanged. The sidecar read is size-bounded *before* the
read (`ladder.rs:382`), which is the right order.

### F-7 — Registry envelope `content_sha256` is asserted, not derived — LOW — PRODUCTION — bounded

`RecordEnvelope.content_sha256` is a caller-supplied field (`record.rs:35-43,54-59`). The local
backend's `read_entry` re-derives and checks `record_id` and `mark_id` from the manifest
(`local/mod.rs:144-155`) but not the content hash; the HTTP backend compares it only against the
request key and says so in a comment (`http.rs:421-424`).

Consequence is bounded: rung 3 returns a record whose manifest's `hard_binding.content_sha256` will
not recompute over the presented audio, so the outcome is `HardMismatch` → `changed`. It cannot
produce a false `verified`. It does compose with F-3 (a hostile registry choosing which anchored
signer gets named) and with F-4 (halting the ladder). No fix proposed: nothing in a manifest commits
to a content hash the registry could be checked against, and the hard binding is what decides.

---

### F-8 — `Manifest::evaluate_hard_binding` is a public method with no production caller — INFO — TEST-ONLY

`grep -rn "evaluate_hard_binding" crates/*/src crates/*/tests` returns the definition
(`audio-provenance-manifest/src/manifest.rs:266`) and three assertions in
`audio-provenance-manifest/tests/interop.rs:49,53,188`. Nothing in `apw_trace`, `audio-provenance-sdk`,
`audio-provenance-cli` or `audio-provenance-wasm` calls it; the ladder reads `hard_binding()` directly
(`ladder.rs:1055`).

It matters because it exposes the exact condition F-1 turned on: it returns `Uncoverable` when
`hard_binding` is `None`, and its doc comment tells a caller that `Uncoverable` "is the case a soft
binding may substitute for" (`manifest.rs:262-265`). No caller acts on that today, so there is no
second instance of F-1. But it is a public method whose contract invites the bug that was just fixed,
and it is the residue of the pre-`BindingEvaluation` design. Delete it, or narrow its visibility, and
drop the sentence.

---

## 3. Checked and found sound

Stated so a reader knows these were examined rather than skipped.

**Signature-before-trust ordering.** `UnverifiedManifest` exposes only `bytes()`,
`declared_schema()` and the unchecked `signature_block()`, and `admit` is its only exit
(`unverified.rs:18-28`). Inside `admit_inner` the order is: schema match, canonical-bytes assert for
the Audio Provenance family, signature block parse, `verify_manifest_signature`, **then** invariants, **then**
every bound field is read (`unverified.rs:96-149`). No accessor that reports a binding, a claim, a
coverage status or an identity exists on the unverified type. `verify_manifest_signature` itself
recomputes the canonical content hash and compares it to `signed_content_hash` *before* the Ed25519
check (`signing.rs:277-293`), so a signature valid over other bytes can never be reported as covering
the manifest in hand.

**Declared schema does not select the validator.** `admit` tries `AudioProvenanceV1` then `ApwV0` and takes
the first that admits (`admission.rs:56-72`), so a document cannot pick a laxer validator by lying.
The declared string is still required to match inside `admit_inner` (`unverified.rs:96-102`).

**Trust chain (`audio-provenance-trust`).** Validity windows are checked against one caller-supplied instant
for the anchor and for every link (`chain.rs:273-301`); the crate reads no clock, and
`audio-provenance-cli/src/context.rs:163-171` is the single reading per run. Revocation is checked *before*
window checks (`chain.rs:270`) and is retroactive by construction — no test consumes the signer's
`signed_at`, and `Refusal::describe` says so in the output (`chain.rs:122-135`). Only the anchor's own
signed list revokes, matched on both `anchor_id` and `issuer_public_key` (`chain.rs:382`). Chain depth
is bounded twice: an absolute `CHAIN_DEPTH_CEILING = 8` checked before each step (`chain.rs:322-326`)
and the anchor's own `max_chain_depth`, itself range-checked to `1..=8` at parse
(`document.rs:197`). Cycles are caught by a visited set (`chain.rs:368`) and self-issuance is refused
at parse (`document.rs:340-346`). Each link's signature is verified under the key the *walk* located,
never under a key the document supplied — stated at `document.rs:291-292` and enforced by
`SignedRecord` having no parse-time signature check. `AnchoredTrustStore` binds on
`proof.public_key_bytes()` (`adapter.rs:41`), correctly, which is exactly what F-2's v0 store does
not.

**Trust document parser.** `Reader::new` requires an exact key set in both directions — unknown keys
and missing keys are both refusals (`reader.rs:20-48`) — so no field can sit outside the signed
payload. Strings are length-bounded and reject control characters *and* bidi overrides and zero-width
joiners (`reader.rs:184-192`), which is the right call: a vouched display name is the entire product,
and U+202E alone would let it render as another studio's. Identifiers are additionally restricted to
`[a-z0-9._-]` and may not start with `.` (`reader.rs:84-97`). Every count is bounded
(`store.rs:16-28`) and the size limit is applied before the read, in the CLI, precisely because
peeking at `format` to choose a loader would otherwise read the bytes first
(`context.rs:115-127`).

**`Instant` grammar.** Fixed-width `YYYY-MM-DDTHH:MM:SSZ`, so byte ordering is chronological ordering
and a window comparison carries no arithmetic (`time.rs:1-7,27-80`). Fractional seconds, offsets and
leap seconds are all refused, with a test that enumerates them (`time.rs:251-267`). The comment
correctly warns that `audio_provenance_registry::SignedAt` accepts the wider grammar and must not be ordered.

**Registry path handling.** `resolve_within` applies a charset whitelist that excludes `..`, absolute
paths and encoded separators by construction, then refuses symlinks and re-checks the canonical parent
against the root (`local/paths.rs:9-69`), with a test enumerating eight traversal shapes plus a
symlink escape (`local/mod.rs:328-367`).

**HTTP backend.** No redirects, no proxy (so no ambient `http_proxy` credentials), no query string, no
`user:pass` authority, http/https only, header size capped, and the body bounded twice — declared
`Content-Length` and the actual read (`http.rs:101-145,163-171,249-293`). 401/403 is `Unauthorized`
and explicitly *not* evidence of absence (`http.rs:223-228`). Retries are bounded and the last
failure keeps its own kind rather than being relabelled (`http.rs:296-324`).

**Outage vs miss.** `Lookup::Unavailable` never becomes a verdict; it sets `incomplete`
(`ladder.rs:546-557`), `NoCandidate(RungCouldNotRun)` mints `not_found` **plus** `incomplete` in one
place so the pair cannot drift (`status.rs:222-229`), and the SDK refuses to return `not_found` from
an unfinished search at all, raising instead (`audio-provenance-sdk/src/verify.rs:234-270`). `--offline`
produces `Skipped`, never `Unavailable`, so it alone never sets `incomplete`
(`ladder.rs:497-508`). An attacker who can cause an outage therefore gets `incomplete`, not
`not_found`, and cannot convert a takedown into "this file was never registered".

**Two records on one locator.** The local backend refuses a second record under an occupied mark at
write time (`local/mod.rs:176-190`); at read time two survivors are `AmbiguousBinding` → `untrusted`,
never a coin flip (`ladder.rs:820-841`, `MarkMatches::is_ambiguous` at `record.rs:236`). The load path
caps an over-full bucket rather than refusing, deliberately, so one bad bucket cannot become a
whole-registry outage (`local/index.rs:31-42`).

**Record swapped after a mark was embedded / replayed onto other audio.** Rung 5 re-derives the
locator from **each returned record's own** declared key and salt over the **received** bytes, before
admission, and a mismatch is terminal `locator_mismatch` (`ladder.rs:736-770`). The comment at
`:741-744` correctly argues why using the unverified declared key there is safe. Squatting another
signer's locator costs a 2^48 search because the key is in the preimage (`locator.rs:90-98`).

**Identity disclosure.** `identity` is populated from `identity_proof_level` and only from it
(`result.rs:547-553`), and that level is `ExternallyVerified` only on `TrustOutcome::Anchored` rows of
the mapping. The property holds by construction, not by review. `NoTrustAnchors` is the default
(`lib.rs:280-281`), and it is not a stub — a verifier with no anchors genuinely knows no identities.

**Container and audio parsers.** RIFF, AIFF, FLAC and ID3v2 walks all use checked arithmetic and a
strictly-increasing advance, so each terminates and none can index past the buffer
(`container.rs:46-208`); the manifest slot is bounded by `MAX_MANIFEST_BYTES` before it is copied
(`container.rs:46-57`). The WAV decoder bounds `frames * channels` against `max_samples` *before* the
`vec!` (`wav.rs:89-103`) and requires `block_align` to equal `channels * bits/8` exactly
(`wav.rs:163-169`), which is what keeps the per-sample slice in range. `ChunkWalker` checks every
declared length against what remains before it is used (`iff.rs:50-94`). `IngestLimits` bounds bytes
and derives a sample ceiling that cannot wrap (`ingest.rs:66-78`), and the doc explains why there is
no wall-clock decode timeout rather than pretending to have one (`ingest.rs:28-33`).

**JUMBF.** Depth capped at 16 and total boxes at 4096, both enforced during the recursive walk
(`jumbf.rs:14-15,67-75`); extended (`length == 1`) and zero lengths are refused rather than guessed;
`offset = end` with `end > offset` guaranteed, so the walk terminates
(`jumbf.rs:84-128`). Unbounded recursion is not reachable.

**Fingerprint index.** `fp.idx` is untrusted input and is treated as such: magic and scheme version
checked, `posting_count * 16` computed with `checked_mul`, the track table required not to overlap the
posting region and to end inside the file, its length required to be a whole number of entries, and
the track count capped at 2^21 — all before any allocation (`fingerprint/index.rs:236-286`). Bucket
bounds are re-checked against the file's own posting count on every query and an over-cap bucket is
not read at all (`index.rs:356-376`). A posting naming an unknown track is a malformed-index error,
not a silent skip (`index.rs:391-395`). A malformed index marks the search `incomplete`; it never
becomes a verdict (`lib.rs:301-320`).

**Advisory scores never reach a verdict.** `AdvisoryScore` is documented as backend-asserted and the
client re-derives the offset histogram locally (`record.rs:245-249`, `fingerprint/index.rs:3-7`).

**JSON depth and integer width.** `UnverifiedManifest::parse` goes through
`audio_provenance_core::parse_signing_input`, never `serde_json::from_slice`, precisely because an integer
literal wider than 64 bits collapses distinct documents onto identical canonical bytes and those bytes
are what is signed (`unverified.rs:38-48`). Proof-value depth is bounded at 64
(`invariants.rs:21`, `validate_proof_values` at `:583`). The trust store's own JSON goes through `serde_json`
with a size bound applied first and a strict key-set reader after (`context.rs:115-128`,
`reader.rs:20-48`).

**Keygen.** `create_new` with mode `0o600`, chosen over `create` specifically because the prior
existence check races (`keygen.rs:74-90`), plus `sync_all`. The non-unix arm has no mode and is
correctly marked as the fallback it is.

---

## 4. Residual risk, stated plainly

- The **v0 trust store is a 64-bit binding** (F-2) and it is the only trust store the published
  npm package and the SDK's path constructor can load. Until F-2 is fixed, treat
  `@writerslogic/audio-provenance-sdk`'s identity output as resting on a 64-bit assumption, not a 128-bit one.
- **A `changed` verdict names an anchored signer on a file they never touched** if the attacker
  supplies the sidecar (F-3). That is the system's loudest false statement and it is reachable by
  anyone holding a published record.
- **The Watermark mark is not an authenticator** (F-5). Every claim that rests on the mark rests on
  the re-derivation of the locator from a signed key and salt; F-1 was the one place that had been
  dropped, and it is now closed.
- **C2PA claims are never cryptographically verified** (F-6). They are diagnostics.
- **Availability**: one appended chunk turns `verified` into `untrusted` (F-4).
- **A documentation error that hides F-1's precondition** survives in
  `audio-provenance-manifest/src/invariants.rs:37-40`: it says the POC JSON Schema "additionally demands
  `export`", and the schema does not. Correcting it is a one-line change in a crate the concurrent run
  owns.
- **Unread**: `crates/apw-watermark-neural`, `training/`, `tools/capture-rig`. Any ONNX model loading, ZIP
  bundle handling or capture-rig input parsing in those trees is unaudited. No such parser exists in
  the crates read here.

---

## 5. Change set

`/Volumes/A/audio-provenance/sdk` is a git repository with nothing committed, so `git diff --stat` is empty
by construction and cannot be used to bound the change set. Stated explicitly instead — this audit
touched exactly three files and nothing owned by the concurrent runs:

| File | Change |
|---|---|
| `crates/apw-trace/src/ladder.rs` | F-1's fix, `evaluate_binding` only, lines 1081-1118. Nothing else in the file moved; verified byte-identical against a pre-experiment backup after the negative control |
| `crates/apw-trace/tests/soft_mark_locator.rs` | New. Two tests demonstrating F-1's surface and its closure |
| `docs/THREAT_MODEL.md` | This document |

Nothing under `crates/apw-watermark-neural`, `training/`, `tools/capture-rig`, `crates/apw-watermark`,
`crates/audio-provenance-sdk`, `crates/audio-provenance-cli`, `crates/audio-provenance-manifest`, `crates/audio-provenance-wasm`,
`packages/sdk` or `crates/audio-provenance-trust` was modified. Nothing under
`/Volumes/A/audio-provenance` was modified; it was read only.
