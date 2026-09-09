// =====================================================================
// PACKAGE NAMES. Published as @writerslogic/audio-provenance-sdk per the target
// repo's committed SCOPE.md (@audio-provenance is unregistered and Audio Provenance/
// Watermark/Trace are third-party marks). The brief's
// `import { verify } from "@audio-provenance/sdk"` is preserved as a workspace
// alias package that re-exports this one, and as the rename target.
// Nothing but package.json name fields encodes the scope.
// =====================================================================

// ---------------------------------------------------------------------
// @writerslogic/audio-provenance-canon  — the interop kernel
// ---------------------------------------------------------------------
export const CANONICALIZATION_ID: "apw-json-sort-v1";

/** JS has one number type; Python does not. The int/float distinction is a
 *  SIGNING INPUT, so it is carried in the type system, not inferred. */
export type CanonValue =
  | null | boolean | string | number | CanonFloat | CanonValue[]
  | { [k: string]: CanonValue };
export interface CanonFloat { readonly __canonFloat: true; readonly value: number }
export declare function f64(value: number): CanonFloat;

/** Byte-exact port of Python
 *  json.dumps(v, sort_keys=True, separators=(",",":"),
 *             ensure_ascii=False, allow_nan=False).encode("utf-8")
 *  Throws CanonicalizationError on NaN/Infinity (allow_nan=False) and on any
 *  non-well-formed UTF-16 string (Python raises UnicodeEncodeError there;
 *  JSON.stringify silently emits an escape, which is the divergence). */
export declare function canonicalJsonBytes(value: CanonValue): Uint8Array;

/** Lexical-preserving parse: keeps the int/float distinction and exact digits,
 *  so parse -> canonicalJsonBytes is byte-identity on any canonical input. */
export declare function parseCanonicalJson(text: string | Uint8Array): CanonValue;

/** Python repr() of a float as json.dumps emits it. VERIFIED DIFFERENTIALLY
 *  against the POC venv, not recalled; the four divergences are:
 *    exponential form: py when exp < -4 or exp >= 16, exponent zero-padded to
 *      2 digits ("1e-05", "1e+16"); js when exp >= 21 or exp <= -7, unpadded
 *    integral floats: py appends ".0" (1.0 -> "1.0"); js does not (-> "1")
 *    large integrals: py 1e16 -> "1e+16"; js -> "10000000000000000"
 *    negative zero: py "-0.0"; js "0"
 *  Mantissa digits agree (both shortest round-trip), so only formatting differs. */
export declare function pythonFloatRepr(value: number): string;

/** apw-json-sort-ascii-v0. NOT A PHANTOM AND NOT A CONVENIENCE SHIM: the POC
 *  computes manifest_signature.signed_content_hash with
 *  json.dumps(m, sort_keys=True, separators=(",",":")).encode() at
 *  generator.py:425 and verify.py:392 — no ensure_ascii argument, so it
 *  DEFAULTS TO True. Confirmed differentially: it emits
 *  {"note":"café 🎵"} where apw-json-sort-v1 emits raw UTF-8.
 *  Required to re-check any POC manifest carrying non-ASCII.
 *  HASH-CHECK ONLY. Never a signing target; no signMappingg accepts it. */
export declare function legacyAsciiJsonBytes(value: CanonValue): Uint8Array;

export declare function sha256(data: Uint8Array): Uint8Array;
export declare function sha256Hex(data: Uint8Array): string;
export declare function sha256File(path: string): Promise<{ hex: string; bytes: number }>;

export const GENESIS: Uint8Array;                     // the 7 ASCII bytes "genesis"
export declare function windowHash(prevHashAscii: string, samples: Float32Array): string;
export declare class HashChain {
  ingest(window: Float32Array): void;
  readonly root: string; readonly length: number;
  verify(events: readonly { prev_hash: string; window_hash: string }[]): Finding[];
}
export declare class CanonicalizationError extends Error {}

// ---------------------------------------------------------------------
// @writerslogic/audio-provenance-manifest
// ---------------------------------------------------------------------
export const SCHEMA_ID: "audio-provenance-manifest-v0";
export const BUNDLE_FORMAT: "apw-evidence-bundle-v1";
export type ProofLevel =
  | "directly_observed" | "inferred" | "user_declared"
  | "externally_verified" | "unknown_unobserved";
export type Severity = "error" | "warning" | "info";
export interface Finding { readonly severity: Severity; readonly code: string; readonly message: string }

export declare function proofRank(level: ProofLevel): number;
/** Returns the LOWEST of the given levels. A claim may never carry a proof
 *  level higher than what was actually observed, so this is a floor, not a max. */
export declare function capProofLevel(...levels: ProofLevel[]): ProofLevel;
export declare function degrade<T>(claim: T, to: ProofLevel, reason: string): Degraded<T>;
export interface Degraded<T> { readonly claim: T; readonly proofLevel: ProofLevel; readonly reason: string }

/** Parsed but NOT yet signature-verified. Structurally prevents reading bound
 *  data before verification: no binding function accepts this type, and the
 *  only way to obtain a Manifest is admitManifest(). */
export interface UnverifiedManifest {
  readonly __unverified: unique symbol;
  readonly bytes: Uint8Array;
  readonly signatureBlock: CanonValue | null;
}
export declare function admitManifest(
  u: UnverifiedManifest, store: TrustStore,
): Promise<{ ok: true; manifest: Manifest } | { ok: false; findings: Finding[] }>;
export declare function validateManifest(value: unknown): Finding[];

// ---------------------------------------------------------------------
// @writerslogic/audio-provenance-apw_watermark
// ---------------------------------------------------------------------
export const ALGORITHM_ID: "apw-watermark-lepqim-v1";
/** 56. The width the CALLER controls. Not 88 (with CRC), not 96 (trellis
 *  steps), not 288 (coded bits) — those are internal and never surface. */
export const MARK_PAYLOAD_BITS: 56;

export type ChannelSupport = "supported" | "degraded" | "unsupported" | "unmeasured";
export interface MarkCapabilities {
  readonly algorithmId: string;
  readonly payloadBits: 56;
  readonly blockSeconds: number;                 // 9.66 at 44.1 kHz
  readonly guaranteedDecodeSeconds: number;      // 19.32
  /** Literally "unsupported". NOT null — null reads as "not yet measured"
   *  and invites hope. Both judge panels called the failure structural. */
  readonly acousticRerecording: "unsupported";
  readonly lossyCompression: ChannelSupport;
  readonly lowpassBelow5kHz: "unsupported";
  readonly timeStretch: "unsupported";
  /** null until a measurement campaign fills it. The SDK never claims
   *  inaudibility it has not measured. */
  readonly measuredTransparency: TransparencyMeasurement | null;
  readonly measuredSurvival: readonly SurvivalMeasurement[] | null;
}
export declare function capabilities(): MarkCapabilities;
export declare function embedMark(src: AudioSource, p: MarkPayload, o?: EmbedOpts): Promise<MarkedAudio>;
export declare function recoverMark(src: AudioSource, o?: RecoverOpts): Promise<MarkRecovery>;
export interface MarkRecovery {
  readonly found: boolean;                       // a miss is found:false, never an error
  readonly payload: MarkPayload | null;
  readonly confidenceClass: "strong" | "single" | "none";
  readonly confidence: number;
  readonly falsePositiveRateAtConfidence: number;  // required; from the bench null test
  readonly blocksTested: number;
  readonly blocksAgreeing: number;
  readonly rateFactor: number | null;
}

// ---------------------------------------------------------------------
// @writerslogic/audio-provenance-sdk  — the published surface
// The brief's snippet compiles against this unchanged.
// ---------------------------------------------------------------------
export type VerifyStatus = "verified" | "changed" | "untrusted" | "not_found";
export type AudioInput = string | URL | Uint8Array | ArrayBuffer | Blob;
export type MatchBasis =
  | "hard_exact"
  | "apw_watermark"
  | "fingerprint"
  /** TRACE_SPEC route 3(c): a CRC-valid Watermark payload naming this record,
   *  corroborated by the record's own signed reference constellation, over a
   *  file whose hard binding no longer recomputes. ALWAYS proofLevel
   *  "inferred", never match 1.0. A fingerprint alone is still "fingerprint"
   *  and still cannot verify. */
  | "mark_and_fingerprint"
  | "none";

export interface VerifyResult {
  /** The four outcomes, identical to daemon/verify.py. */
  readonly status: VerifyStatus;

  /** REQUIRED INVARIANT: non-null IF AND ONLY IF
   *  identityProofLevel === "externally_verified".
   *  A self-generated key proves KEY POSSESSION, never identity, so it yields
   *  null here. This is why the type is nullable and not string. */
  readonly identity: string | null;

  /** UTC calendar date of the signature, YYYY-MM-DD. Null when nothing signed. */
  readonly signedAt: string | null;

  /** Binding strength in [0,1]. === 1.0 IFF matchBasis is "hard_exact";
   *  soft bases cap at 0.99. 0 on not_found. */
  readonly match: number;
  readonly matchBasis: MatchBasis;

  readonly identityProofLevel: ProofLevel;
  readonly identityAuthority: string | null;     // null whenever identity is null
  readonly binding: BindingReport;
  readonly signature: SignatureReport | null;
  readonly trace: readonly RecoveryStep[];       // every rung that ran, in order
  readonly recovery: RecoveryReport;             // discarded candidates, exhausted flags
  /** Locator recovery and recording association are separate by construction. A copied mark can
   *  recover a record while recordingAssociation.status is "rejected" and the overall status is
   *  "changed". */
  readonly markRecovery: MarkRecoveryReport;
  readonly recordingAssociation: RecordingAssociationReport;
  readonly findings: readonly Finding[];
  readonly manifest: Manifest | null;
  readonly registry: RegistrySource | null;
  readonly contentSha256: string;                // always populated
  /** True when a rung could not run (registry unreachable, decode timeout).
   *  A network fault sets this; it never becomes a verdict. */
  readonly incomplete: boolean;
}

export interface BindingReport {
  readonly kind:
    | "hard_hash"
    | "soft_apw_watermark"
    | "soft_fingerprint"
    | "soft_mark_and_fingerprint"
    | "none";
  readonly match: number;
  /** hard_hash -> directly_observed; a `strong` apw_watermark -> directly_observed;
   *  a `single` apw_watermark, any fingerprint, and soft_mark_and_fingerprint ->
   *  inferred, never higher. */
  readonly proofLevel: ProofLevel;
  /** The threshold the reported `match` was actually gated against, which is
   *  NOT always softBindingThreshold: a soft_mark_and_fingerprint match is a
   *  local-region-coverage figure gated against the reference-constellation floor
   *  (0.80), the maximum interior-gap rule, discontinuity rejection, and signed
   *  duration/block-distribution consistency. Printing one number beside another
   *  basis's threshold is a confidently
   *  wrong sentence. Default 0.72 on every other basis. */
  readonly threshold: number;
  /** REQUIRED non-null for every soft basis. A confidence with no
   *  false-positive rate is not actionable. Sourced from the bench null
   *  test, not a formula — so soft `verified` cannot ship before it passes. */
  readonly falsePositiveRateAtMatch: number | null;
  readonly expectedSha256: string | null;
  readonly observedSha256: string;
  readonly detail: string;
}

/** Finding a signed record is not evidence that the presented recording is that record. */
export interface MarkRecoveryReport {
  readonly recovered: boolean;
  readonly located: boolean;
  readonly method: RecoveryMethod | null;
  readonly recoveredViaMark: boolean;
  readonly recordLocatorMatched: boolean;
  readonly proofLevel: ProofLevel;
}

export type RecordingAssociationStatus =
  | "exact"
  | "associated"
  | "rejected"
  | "insufficient_evidence"
  | "not_evaluated";

export interface RecordingAssociationReport {
  readonly status: RecordingAssociationStatus;
  readonly established: boolean;
  readonly proofLevel: ProofLevel;
  readonly match: number;
}

export type RecoveryMethod =
  | "embedded_manifest" | "sidecar_manifest" | "content_hash_lookup"
  | "decoded_audio_hash_lookup" | "apw_watermark_recovery" | "fingerprint_search";
export interface RecoveryStep {
  readonly method: RecoveryMethod;
  readonly outcome: "hit" | "miss" | "skipped" | "degraded" | "unavailable";
  readonly proofLevel: ProofLevel;
  readonly durationMs: number;
  readonly detail: string;
}

export interface VerifyOptions {
  /** A configured registry NAME (e.g. "public") or a backend instance.
   *  Never a URL constant; names resolve through discovered config. */
  readonly registry?: string | RegistryBackend;
  readonly config?: AudioProvenanceConfig | string;
  readonly trustStore?: TrustStore | string;
  readonly sidecar?: string | false;
  readonly offline?: boolean;                    // registry rungs become "skipped"
  readonly softBindingThreshold?: number;        // default 0.72
  /** Enables the fingerprint rung to emit a candidate. Default false; it can
   *  never produce `verified`. */
  readonly acceptInferredAssociation?: boolean;
  readonly decoder?: DecoderPort;
  readonly signal?: AbortSignal;
}

export declare function verify(input: AudioInput, options?: VerifyOptions): Promise<VerifyResult>;
export declare function sign(input: AudioInput, options: SignOptions): Promise<SignResult>;
export declare function embed(input: AudioInput, options: EmbedOptions): Promise<EmbedResult>;
/** Reports what Trace found, with no verdict and no trust evaluation. */
export declare function inspect(input: AudioInput, options?: InspectOptions): Promise<InspectReport>;

export interface SignOptions {
  readonly key: SigningKey | string;
  readonly manifest?: ManifestDraft;
  readonly embed?: boolean;                      // write into a WAV `aprv` chunk
  readonly sidecar?: string | false;             // default "<file>.audio-provenance.json"
  /** Order is enforced: mark, then hash, then sign. Passing a mark against an
   *  already-signed input throws `mark_after_sign`, because marking changes
   *  the audio the hard binding covers. */
  readonly mark?: { namespace?: number; delta?: number } | false;
  readonly out?: string;
  readonly registry?: string | RegistryBackend;
  readonly signal?: AbortSignal;
}
export interface SignResult {
  readonly manifest: Manifest;
  readonly manifestBytes: Uint8Array;            // EXACTLY the bytes that were signed
  readonly contentSha256: string;
  readonly signature: SignatureReport;
  readonly sidecarPath: string | null;
  readonly embeddedIn: string | null;
  readonly mark: MarkReport | null;
  readonly receipt: RegistryReceipt | null;
}
