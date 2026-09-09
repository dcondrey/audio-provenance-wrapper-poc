# @writerslogic/audio-provenance-sdk

Audio provenance you can verify from the file alone.

```ts
import { verify } from "@writerslogic/audio-provenance-sdk";

const result = await verify(audioFile);

result.status;   // "verified" | "changed" | "untrusted" | "not_found"
result.identity; // "Signal Room Studios"
result.signedAt; // "2026-03-14"
result.match;    // 1.0
```

That snippet is `examples/verify.js`. Run it:

```
pnpm --filter @writerslogic/audio-provenance-sdk example
```

```
verified
Signal Room Studios
2026-08-31
1
```

Node >= 20, ESM only, strict TypeScript. The engine is the Rust `audio-provenance-sdk` compiled to
WebAssembly, so the verdict here and the verdict from the `audio-provenance` CLI come out of the same
status mapping, the same soft-binding gates and the same signature checks.

## Install

```
pnpm add @writerslogic/audio-provenance-sdk
```

## `verify(input, options?)`

`input` is a path, a `URL`, a `Uint8Array`, a `Buffer`, an `ArrayBuffer`, or a `Blob` / `File` from
a browser `<input type="file">`. Paths are readable in Node only; the browser entry point raises
`input_unreadable` rather than pretending a filesystem exists.

```ts
const result = await verify(file, {
  trustStore,   // a audio-provenance-trust-store-v0 document: without one, identity is always null
  nullTest,     // a audio-provenance-bench null-test report: without one, no soft binding verifies
  records,      // the registry, as record envelopes you fetched yourself
  registryName: "public",
  offline: false,
  softBindingThreshold: 0.72,
  acceptInferredAssociation: false,
});
```

A misspelled option is an error, not a silent no-op. `{ trustStoer }` throws a `AudioProvenanceError` with
`code === "option_invalid"`, because a caller who believes a trust store is in force when it is not
is reading a different verdict from the one they think they asked for.

## The result

`VerifyResult` mirrors the Rust `apw_trace::result::VerifyResult` field for field, with keys
camelCased. The parts worth reading before you write a UI:

| Field | Type | Meaning |
|---|---|---|
| `status` | `"verified" \| "changed" \| "untrusted" \| "not_found"` | four members, and there is no fifth |
| `reason` | `string` | the status mapping's own code, e.g. `hard_binding_match` |
| `identity` | `string \| null` | **`null` unless a trust anchor resolved the key to a name** |
| `identityProofLevel` | `ProofLevel` | `externally_verified` exactly when `identity` is non-null |
| `signedAt` | `string \| null` | `YYYY-MM-DD` |
| `match` | `number \| null` | `null` when no binding was evaluated |
| `matchBasis` | `"hard_exact" \| "apw_watermark" \| "fingerprint" \| "mark_and_fingerprint" \| "none"` | which binding carried the verdict |
| `binding` | `BindingReport` | the numbers behind `match`, including the false-positive rate |
| `method` | `RecoveryMethod \| null` | which rung recovered the record |
| `trace` | `RecoveryStep[]` | all six rungs, with outcomes and timings |
| `incomplete` | `boolean` | a rung that was meant to run and could not |

### `identity` is usually `null`, and that is the correct answer

A valid self-generated Ed25519 signature proves **key possession**. It says nothing about who holds
the key. So an unanchored signer verifies as `untrusted` with `identity: null` even though the
signature is cryptographically perfect, and `identity` is never the empty string standing in for a
name nobody vouched for. Pass a `trustStore` to get a name; `signature.valid` is the separate
question of whether the bytes were signed by the key the manifest declares.

### `match === 1` is an exact test for exact bytes

`1.0` is reachable only through a hard binding. Every soft basis is clamped to `0.99`, so you can
branch on `match === 1` without remembering a convention.

`match` is `null`, not `0`, when `matchBasis === "none"`: `0` reads as "measured, and nothing
matched", when the truth is that no binding was evaluated at all. The unmodified Rust number is
always at `binding.match`. Those two normalisations (`match`, and `""` → `null` for `identity`) are
the only places this package differs from the raw Rust JSON, and both remove a claim rather than
add one.

### `mark_and_fingerprint` is how a transcode still verifies

A lossy transcode destroys the hard binding. It can still reach `verified` at proof level
`inferred` when BOTH a CRC-valid Watermark payload naming the record decodes out of the audio AND
the record's own signed reference constellation affirms it. A fingerprint alone never verifies, and
a payload that decodes while the constellation disagrees is the watermark-copy attack and stays
`changed`.

That verdict also needs a measured false-positive rate. Without `options.nullTest`, the same file
comes back `untrusted` with `reason: "soft_binding_false_positive_rate_unknown"` — the soft binding
did affirm, so calling the file tampered would be a different false statement in a new place.

### Acoustic re-recording is unsupported

Audio played through a loudspeaker and captured by a microphone returns `not_found`.
`capabilities().acousticRerecording` is the literal string `"unsupported"`, never `null` and never
absent. Do not describe a Audio Provenance mark as surviving playback in a room or a phone recording.

## The registry

WebAssembly has no filesystem and no socket, so the `local` and `http` backends the native SDK
ships cannot run here. What a browser or a Node process CAN do is fetch the records itself and hand
them over, which is `options.records`. It is a real backend, not a stub: same non-emptiness rule,
same ambiguity reporting, same envelope validation as the filesystem one.

A file carrying an embedded manifest needs no registry at all. When you do need a remote one,
`RegistryBackend` is synchronous and `fetch` is not, so it is two calls rather than a fake
synchronous one:

```ts
import { locators, verify } from "@writerslogic/audio-provenance-sdk";

const { locators: found } = await locators(file);
const records = await Promise.all(
  found.map((m) => fetch(`https://registry.example/marks/${m.locatorHex}`).then((r) => r.json())),
);
const result = await verify(file, { records, trustStore, nullTest });
```

A locator is an **index**, not an identity: several records can share one, and recovering one proves
nothing on its own. Only `verify` produces a verdict.

## What this build can do

`capabilities()` answers for this build, not for the native one:

| Capability | Here |
|---|---|
| Decode wav / aiff / mp3 / flac / ogg / mp4 from bytes | yes |
| Rung 1, embedded manifest | yes |
| Rung 2, sidecar manifest | no — there is no directory to resolve one against |
| Rungs 3–6, over `options.records` | yes |
| `local` / `http` registry backends | no |
| Trust store, null-test report | yes, passed in as JSON rather than read from a path |
| Signing, marking, publishing | no — the producer surface is native-only, use the `audio-provenance` CLI |

## Errors

Every error is a `AudioProvenanceError` carrying the Rust side's own stable `code`, the same vocabulary the
CLI exits with:

```ts
import { AudioProvenanceError, verify } from "@writerslogic/audio-provenance-sdk";

try {
  await verify(file, options);
} catch (error) {
  if (error instanceof AudioProvenanceError && error.code === "registry_unavailable") {
    // A rung was meant to run and could not. This is deliberately NOT `not_found`: a caller reads
    // `not_found` as a fact about the work, and a dropped connection does not earn that reading.
  }
}
```

## Browser

```ts
import { verify } from "@writerslogic/audio-provenance-sdk/browser";
```

Reached automatically through the `browser` export condition. Your bundler must be able to load
`.wasm`; every current one can. The module is about 2.2 MB.

## Development

```
pnpm install
pnpm --filter @writerslogic/audio-provenance-sdk run build:wasm   # needs wasm-pack and the wasm32-unknown-unknown target
pnpm --filter @writerslogic/audio-provenance-sdk run build
pnpm --filter @writerslogic/audio-provenance-sdk test
```

`test/fixtures` holds a real signed WAV, the marked-and-registered WAV it came from, a 128k mp3
transcode of that, an unregistered original, the registry record, a trust store and a bench
null-test report. Every test drives the published entry point over those bytes; nothing is mocked.
