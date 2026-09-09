/**
 * The result shape, mirrored from the Rust `apw_trace::result::VerifyResult`.
 *
 * Every union below is the exact set of strings the Rust enum serialises. Widening one of these to
 * `string` would let a caller branch on a status that cannot occur and, worse, would stop the
 * compiler flagging the day a new one is added.
 */

/** `audio_provenance_core::VerificationStatus`. Four members, and there is no fifth. */
export type VerificationStatus = "verified" | "changed" | "untrusted" | "not_found";

/** `audio_provenance_core::ProofLevel`. */
export type ProofLevel =
  | "directly_observed"
  | "inferred"
  | "user_declared"
  | "externally_verified"
  | "unknown_unobserved";

/**
 * What produced the match.
 *
 * `mark_and_fingerprint` is route 3(c): the signed hard binding did not recompute, and BOTH a
 * CRC-valid Watermark payload naming this record and the record's own signed reference constellation
 * say this is the same work over a lossy path. It is deliberately distinct from `fingerprint`,
 * because a fingerprint alone never verifies.
 */
export type MatchBasis =
  | "hard_exact"
  | "apw_watermark"
  | "fingerprint"
  | "mark_and_fingerprint"
  | "none";

export type BindingKind =
  | "hard_hash"
  | "soft_apw_watermark"
  | "soft_fingerprint"
  | "soft_mark_and_fingerprint"
  | "none";

export type RecoveryMethod =
  | "embedded_manifest"
  | "sidecar_manifest"
  | "content_hash_lookup"
  | "decoded_audio_hash_lookup"
  | "apw_watermark_recovery"
  | "fingerprint_search";

export type StepOutcome = "hit" | "miss" | "skipped" | "degraded" | "unavailable";

export type Severity = "info" | "warning" | "error";

/**
 * Whether a false-positive rate is an observation or a bound on an observation of zero. A reported
 * `0.0` would be a claim no trial count can support, so zero accepts are published as the
 * rule-of-three 95% upper bound instead.
 */
export type RateBasis = "observed" | "upper_bound_95";

export interface BindingReport {
  readonly kind: BindingKind;
  /**
   * The raw score, never null. `1.0` is reachable only through a hard binding; every soft basis is
   * clamped to `0.99`, which makes `binding.match === 1` an exact test for exact bytes.
   */
  readonly match: number;
  readonly proofLevel: ProofLevel;
  /** The threshold this score was actually gated against, which differs by basis. */
  readonly threshold: number;
  /** Non-null for every soft basis that could have reached `verified`. */
  readonly falsePositiveRateAtMatch: number | null;
  readonly falsePositiveRateBasis: RateBasis | null;
  readonly expectedSha256: string | null;
  readonly observedSha256: string;
  readonly detail: string;
}

export interface SignatureReport {
  readonly algorithm: string;
  readonly canonicalization: string;
  readonly signerId: string | null;
  /** Key possession, and nothing more. A valid signature says who holds a key, not who they are. */
  readonly valid: boolean;
}

export interface RecoveryStep {
  readonly method: RecoveryMethod;
  readonly outcome: StepOutcome;
  readonly proofLevel: ProofLevel;
  readonly durationMs: number;
  readonly detail: string;
}

export interface DiscardedCandidate {
  readonly method: RecoveryMethod;
  readonly recordDigest: string | null;
  readonly reason: string;
  readonly detail: string;
}

export interface RecoveryReport {
  /** False means the search stopped early, which is why `not_found` never means "none exists". */
  readonly exhausted: boolean;
  readonly discarded: readonly DiscardedCandidate[];
  readonly diagnostics: readonly Finding[];
}

/** Locator recovery is not recording authentication. */
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

export interface Finding {
  readonly severity: Severity;
  readonly code: string;
  readonly path: string;
  readonly message: string;
}

export interface RegistrySourceReport {
  readonly name: string;
  /** `filesystem`, `http`, or `memory` for records the caller supplied. */
  readonly kind: string;
  readonly location: string;
}

export interface VerifyResult {
  readonly status: VerificationStatus;
  /** The status mapping's own reason code, e.g. `hard_binding_match`. */
  readonly reason: string;
  /**
   * The name a configured trust anchor resolved the signing key to, or `null`.
   *
   * `null` is the common and correct answer. A valid self-generated Ed25519 signature proves KEY
   * POSSESSION and says nothing about who holds the key, so an unanchored signer has no identity
   * here even though the signature is cryptographically perfect. Never the empty string.
   */
  readonly identity: string | null;
  readonly identityProofLevel: ProofLevel;
  readonly identityAuthority: string | null;
  readonly signedAt: string | null;
  /**
   * The binding strength, or `null` when no binding was evaluated at all.
   *
   * The Rust reports `0.0` in that case; this surface reports `null`, because `0` reads as "nothing
   * matched by this much" when the truth is that nothing was measured. `match === null` holds
   * exactly when `matchBasis === "none"`. The unmodified number is always at `binding.match`.
   */
  readonly match: number | null;
  readonly matchBasis: MatchBasis;
  readonly binding: BindingReport;
  readonly signature: SignatureReport | null;
  readonly trace: readonly RecoveryStep[];
  readonly recovery: RecoveryReport;
  readonly markRecovery: MarkRecoveryReport;
  readonly recordingAssociation: RecordingAssociationReport;
  readonly findings: readonly Finding[];
  readonly registry: RegistrySourceReport | null;
  readonly contentSha256: string;
  readonly contentBytes: number;
  readonly decodedAudioSha256: string | null;
  readonly method: RecoveryMethod | null;
  /** A rung that was meant to run and could not. A skipped rung does not set this. */
  readonly incomplete: boolean;
}

export interface InspectResult {
  /** `wav`, `aiff`, `mp3`, `flac`, `ogg` or `mp4`. */
  readonly container: string;
  readonly contentSha256: string;
  readonly contentBytes: number;
  readonly decodedAudioSha256: string | null;
  readonly sampleRate: number;
  readonly channels: number;
  readonly durationSeconds: number;
  readonly manifestRecovered: boolean;
  readonly manifestSchema: string | null;
  readonly method: RecoveryMethod | null;
  readonly signerId: string | null;
  readonly trace: readonly RecoveryStep[];
  readonly recovery: RecoveryReport;
  readonly incomplete: boolean;
}

export interface LocatorMatch {
  readonly version: number;
  readonly namespace: number;
  /** 12 lowercase hex characters. An INDEX into a registry, never an identity. */
  readonly locatorHex: string;
  readonly confidenceClass: "strong" | "single" | "none";
  readonly blocksAccepted: number;
}

export interface LocatorsResult {
  readonly contentSha256: string;
  readonly locators: readonly LocatorMatch[];
}

/** What this build can do, with the wasm platform limits stated rather than implied. */
export interface PlatformCapabilities {
  readonly target: string;
  readonly filesystem: boolean;
  readonly network: boolean;
  readonly sidecarManifest: boolean;
  readonly registryBackends: readonly string[];
  readonly signing: boolean;
  readonly marking: boolean;
  readonly note: string;
}

export interface Capabilities {
  readonly sdk: string;
  readonly version: string;
  readonly sampleRateHz: number;
  readonly canonicalization: string;
  readonly manifestSchemas: readonly string[];
  readonly signatureAlgorithms: readonly string[];
  readonly recoveryMethods: readonly RecoveryMethod[];
  readonly registryBackends: readonly string[];
  readonly decodes: readonly string[];
  readonly encodes: readonly string[];
  readonly mark: Readonly<Record<string, unknown>>;
  readonly fingerprint: Readonly<Record<string, unknown>>;
  /** The literal string `"unsupported"`, never null and never absent. */
  readonly acousticRerecording: string;
  readonly defaultSoftBindingThreshold: number;
  readonly softBindingVerifiedRequiresNullTest: boolean;
  readonly platform: PlatformCapabilities;
}

/** A registry record envelope, `audio-provenance-registry-record-v1`, exactly as a registry stores it. */
export type RegistryRecordEnvelope = Readonly<Record<string, unknown>>;

export interface VerifyOptions {
  /**
   * The records this verification may consult.
   *
   * There is no filesystem and no socket in a wasm build, so this is the registry. Fetch the
   * records in JavaScript, where fetching is asynchronous and natural, and hand them over. Use
   * {@link locators} when you need to know which records to fetch.
   */
  readonly records?: readonly RegistryRecordEnvelope[];
  /** The name the result reports as the answering registry. Defaults to `caller`. */
  readonly registryName?: string;
  /**
   * A `audio-provenance-trust-store-v0` document. Without one, `identity` is always `null`: a verifier with
   * no configured anchors genuinely knows no identities, and saying so is the correct answer.
   */
  readonly trustStore?: Readonly<Record<string, unknown>>;
  /**
   * A `audio-provenance-bench` null-test report. Without one, no soft binding reaches `verified`, because
   * the false-positive rate such a verdict must publish has no honest value until a bench has run.
   */
  readonly nullTest?: Readonly<Record<string, unknown>>;
  readonly offline?: boolean;
  readonly softBindingThreshold?: number;
  /** Lets the fingerprint rung emit a candidate. It can still never produce `verified`. */
  readonly acceptInferredAssociation?: boolean;
}
