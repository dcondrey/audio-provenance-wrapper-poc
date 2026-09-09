AUDIO PROVENANCE SDK — ARCHITECTURE

Repo: /Volumes/A/audio-provenance/sdk (exists; contains .gitignore, .npmrc, pnpm-workspace.yaml globbing packages/*, and SCOPE.md). pnpm workspace, TypeScript 5.x, Node >= 20, ESM-only. No Python.

NPM SCOPE — RESOLVED AGAINST THE BRIEF. The brief specifies `import { verify } from "@audio-provenance/sdk"`. SCOPE.md, already committed in the target repo, states that `@audio-provenance` is unregistered and that Audio Provenance/Watermark/Trace are third-party marks, so publishing there invites an npm dispute-policy transfer. SCOPE.md wins: packages ship as `@writerslogic/audio-provenance-*`, public entry `@writerslogic/audio-provenance-sdk`. The user's snippet is preserved verbatim as the rename target and as a local alias via a tsconfig `paths` entry plus a workspace `"@audio-provenance/sdk": "workspace:*"` alias package that re-exports the real one. Nothing but package.json `name` fields and workspace specifiers encodes the scope.

EIGHT PACKAGES, LAYERED. Arrows are compile-time dependencies.

  canon  (zero deps)
    |
    +--> audio ------+
    +--> manifest ---+---> crypto ---+
                     |               |
                     +--> apw_watermark   +--> registry
                                     |
                                     +--> sdk (contains apw_trace/) --> cli

1. @writerslogic/audio-provenance-canon — the cross-language interop kernel. Byte-exact port of apw-json-sort-v1, a lexical-preserving JSON parser that keeps the int/float distinction, the Python-float repr table, SHA-256 helpers, and the APW hash chain. Zero runtime dependencies (node:crypto + TextEncoder only). Every signature in the system is a signature over bytes this package produced, so it is the one package tested against the Python POC rather than against itself. Also exports apw-json-sort-ascii-v0 as a hash-check-only function (see below).

2. @writerslogic/audio-provenance-audio — container and PCM layer. WAV (RIFF/RF64) and AIFF/AIFF-C parse+write in-package; MP3/M4A/FLAC/Ogg/Opus header parse in-package but decode through an injected DecoderPort (no bundled codec). Produces the mono-summed Float32Array windows the hash chain and Watermark both consume. Owns chunk-level read/write so a signed manifest can be embedded in a WAV without disturbing the audio the hard binding covers.

3. @writerslogic/audio-provenance-manifest — the audio-provenance-manifest-v0 vocabulary: types, builder, and a direct port of daemon/schema.py validate_manifest_invariants. Owns the ProofLevel lattice and the rule that governs the whole system: a claim may never carry a proof level higher than what was actually observed. No crypto, no I/O.

4. @writerslogic/audio-provenance-crypto — Ed25519 over node:crypto only (raw 32-byte keys via the JWK OKP path), X.509 chain validation via node:crypto X509Certificate, TrustStore, revocation. Owns the rule that an identity proof level is read from the anchor and never upgraded locally.

5. @writerslogic/audio-provenance-apw_watermark — the watermark. Embed, recover, payload codec, capabilities. Depends on canon (CRC, payload framing) and audio (PCM). Knows nothing about manifests or registries; it moves 56 bits.

6. @writerslogic/audio-provenance-registry — RegistryBackend interface plus FilesystemRegistry (default, content-addressed, append-only), HttpRegistry (base URL from config, never a constant), MemoryRegistry, ChainRegistry. Owns config discovery so a registry NAME like "public" resolves through the user's config. Enforces the rule that keeps a network fault from becoming a verdict: a miss returns null, an unreachable or malformed backend throws RegistryUnavailableError.

7. @writerslogic/audio-provenance-sdk — the public facade plus Trace. verify/sign/embed/inspect are the entire published surface. Trace lives at src/apw-trace/ as an internal orchestrator rather than its own package, because it has no consumer independent of verify(). Re-exports the types a normal consumer needs so one install suffices.

8. @writerslogic/audio-provenance-cli — the `audio-provenance` binary. node:util parseArgs, no argument-parsing dependency, no provenance logic: every verdict comes from the SDK; the CLI chooses a renderer and maps status to an exit code.

WHY TRACE IS NOT A PACKAGE. It is the only consumer of the ladder and the only producer of VerifyResult.trace. Splitting it would create a package whose sole export is consumed by exactly one caller and whose types are re-exported wholesale — a parallel abstraction. It is a directory.

THE ONE CROSS-CUTTING RULE. Signature verification runs before any bound data is read, in both the manifest seal and the bundle index seal. In TypeScript this is enforced structurally, not by convention: the parsed-but-unverified manifest is typed `UnverifiedManifest` (an opaque branded type exposing only `bytes` and `signatureBlock`), and the only function that produces a `Manifest` is `admitManifest(unverified, store)`, which returns `Manifest | Rejection`. No binding-evaluation function accepts an `UnverifiedManifest`. A reviewer can grep for the brand and see the invariant is total.
