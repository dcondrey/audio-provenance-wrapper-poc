import type {
  AudioInput,
  ExternalSigner,
  SignedRecord,
  SignOptions,
  Capabilities,
  InspectResult,
  LocatorsResult,
  VerificationStatus,
  VerifyOptions,
  VerifyResult,
} from "./public.js";

/**
 * An error the Rust side raised, carrying that side's own stable code.
 *
 * The code is never invented here: it is `audio_provenance_core::CodedError::code`, the same vocabulary the
 * `audio-provenance` CLI exits with, so branching on it in JavaScript and branching on it in Rust mean the
 * same thing.
 */
export class AudioProvenanceError extends Error {
  readonly code: string;

  constructor(code: string, message: string, options?: { cause?: unknown }) {
    super(message, options);
    this.name = "AudioProvenanceError";
    this.code = code;
  }
}

/** The functions `wasm-pack` generates. Both the node and the bundler build satisfy it. */
export interface AudioProvenanceWasm {
  verifyBytes(audio: Uint8Array, optionsJson?: string | null): string;
  inspectBytes(audio: Uint8Array, optionsJson?: string | null): string;
  capabilities(): string;
  prepareSigning(audio: Uint8Array, optionsJson: string): string;
  sealManifest(unsignedManifest: string, publicKeyHex: string, signatureHex: string): Uint8Array;
  locators(audio: Uint8Array): string;
  statuses(): string[];
  version(): string;
}

/** Reads a path or URL into bytes. Absent in a browser build, where there is no filesystem. */
export type PathReader = (source: string | URL) => Promise<Uint8Array>;

export interface Runtime {
  readonly wasm: AudioProvenanceWasm;
  readonly readPath?: PathReader;
}

const rethrow = (error: unknown): never => {
  if (error instanceof AudioProvenanceError) {
    throw error;
  }
  const code =
    typeof error === "object" && error !== null && typeof (error as { code?: unknown }).code === "string"
      ? (error as { code: string }).code
      : "sdk_call_failed";
  throw new AudioProvenanceError(code, error instanceof Error ? error.message : String(error), {
    cause: error,
  });
};

async function toBytes(input: AudioInput, readPath?: PathReader): Promise<Uint8Array> {
  if (input instanceof Uint8Array) {
    return input;
  }
  if (input instanceof ArrayBuffer) {
    return new Uint8Array(input);
  }
  if (ArrayBuffer.isView(input)) {
    return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  }
  if (typeof Blob !== "undefined" && input instanceof Blob) {
    return new Uint8Array(await input.arrayBuffer());
  }
  if (typeof input === "string" || input instanceof URL) {
    if (!readPath) {
      throw new AudioProvenanceError(
        "input_unreadable",
        "this build has no filesystem, so a path cannot be read; pass a File, Blob, ArrayBuffer or Uint8Array instead",
      );
    }
    return readPath(input);
  }
  throw new AudioProvenanceError(
    "input_unreadable",
    "expected a path, URL, Uint8Array, ArrayBuffer, ArrayBufferView, Blob or File",
  );
}

const toHex = (bytes: Uint8Array): string =>
  Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");

const fromHex = (hex: string): Uint8Array =>
  Uint8Array.from(hex.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));

/**
 * Adapts a WebCrypto Ed25519 private key handle. The handle can be non-extractable, and the
 * package only ever calls `subtle.sign` on it.
 */
export function webCryptoSigner(privateKey: CryptoKey, publicKey: Uint8Array): ExternalSigner {
  if (publicKey.length !== 32) {
    throw new AudioProvenanceError("option_invalid", "an Ed25519 public key is 32 bytes");
  }
  return {
    publicKeyHex: toHex(publicKey),
    sign: async (digest) => new Uint8Array(await globalThis.crypto.subtle.sign("Ed25519", privateKey, digest as BufferSource)),
  };
}

const parse = <T>(json: string): T => JSON.parse(json) as T;

/**
 * Applies the two places this surface deliberately differs from the raw Rust JSON, and nothing
 * else. Both differences remove a claim rather than add one.
 */
function settle(raw: VerifyResult): VerifyResult {
  const identity = raw.identity === "" ? null : raw.identity;
  // The Rust reports `0.0` when no binding was evaluated. `0` reads as a measurement; `null` says
  // nothing was measured, which is what happened. `binding.match` keeps the unmodified number.
  const score = raw.matchBasis === "none" ? null : raw.match;
  return { ...raw, identity, match: score };
}

export function createClient(runtime: Runtime) {
  const { wasm, readPath } = runtime;

  const options = (given?: VerifyOptions): string | undefined =>
    given === undefined ? undefined : JSON.stringify(given);

  return {
    /** Verifies a file, some bytes, or a `File` picked in a browser. */
    async verify(input: AudioInput, given?: VerifyOptions): Promise<VerifyResult> {
      const bytes = await toBytes(input, readPath);
      try {
        return settle(parse<VerifyResult>(wasm.verifyBytes(bytes, options(given))));
      } catch (error) {
        return rethrow(error);
      }
    },

    /** Reports what the recovery ladder found, with no verdict. */
    async inspect(input: AudioInput, given?: VerifyOptions): Promise<InspectResult> {
      const bytes = await toBytes(input, readPath);
      try {
        return parse<InspectResult>(wasm.inspectBytes(bytes, options(given)));
      } catch (error) {
        return rethrow(error);
      }
    },

    /**
     * The Watermark locators recoverable from this audio.
     *
     * `RegistryBackend` is synchronous and `fetch` is not, so a caller backed by a remote registry
     * runs this first, fetches the records for the locators it returns, and passes them to
     * {@link verify} as `options.records`.
     */
    async locators(input: AudioInput): Promise<LocatorsResult> {
      const bytes = await toBytes(input, readPath);
      try {
        return parse<LocatorsResult>(wasm.locators(bytes));
      } catch (error) {
        return rethrow(error);
      }
    },

    /**
     * Signs with a caller-supplied signer. The package holds no private key: it prepares the
     * record, the signer signs the digest, and the package verifies that signature under the
     * declared public key before it returns a record.
     */
    async sign(input: AudioInput, signer: ExternalSigner, given: SignOptions = {}): Promise<SignedRecord> {
      const bytes = await toBytes(input, readPath);
      const publicKeyHex = signer?.publicKeyHex;
      if (typeof publicKeyHex !== "string" || !/^[0-9a-f]{64}$/.test(publicKeyHex)) {
        throw new AudioProvenanceError(
          "option_invalid",
          "signer.publicKeyHex must be 64 lowercase hex characters",
        );
      }
      if (typeof signer.sign !== "function") {
        throw new AudioProvenanceError("option_invalid", "signer.sign must be a function");
      }
      const signedAt = given.signedAt ?? new Date().toISOString().replace(/\.\d+Z$/, "Z");
      try {
        const prepared = parse<{
          unsignedManifest: string;
          algorithm: string;
          digestHex: string;
          signerId: string;
          locator: string;
          locatorSalt: string;
          contentSha256: string;
          contentBytes: number;
          decodedAudioSha256: string;
        }>(
          wasm.prepareSigning(
            bytes,
            JSON.stringify({ publicKeyHex, signedAt, locatorSaltHex: given.locatorSaltHex }),
          ),
        );
        const digest = fromHex(prepared.digestHex);
        let produced: Uint8Array | ArrayBuffer;
        try {
          produced = await signer.sign(digest);
        } catch (error) {
          throw new AudioProvenanceError("signer_failed", "the supplied signer failed", { cause: error });
        }
        const signature =
          produced instanceof ArrayBuffer ? new Uint8Array(produced) : (produced as Uint8Array);
        if (!(signature instanceof Uint8Array) || signature.length !== 64) {
          throw new AudioProvenanceError(
            "signer_output_invalid",
            "the supplied signer must return a 64-byte Ed25519 signature",
          );
        }
        const record = wasm.sealManifest(prepared.unsignedManifest, publicKeyHex, toHex(signature));
        return {
          record,
          registryRecord: {
            content_sha256: prepared.contentSha256,
            manifest: JSON.parse(new TextDecoder().decode(record)) as unknown,
            record_format: "audio-provenance-registry-record-v1",
            signed_at: signedAt,
          },
          signerId: prepared.signerId,
          locator: prepared.locator,
          locatorSalt: prepared.locatorSalt,
          contentSha256: prepared.contentSha256,
          contentBytes: prepared.contentBytes,
          decodedAudioSha256: prepared.decodedAudioSha256,
          signedAt,
        };
      } catch (error) {
        return rethrow(error);
      }
    },

    capabilities(): Capabilities {
      try {
        return parse<Capabilities>(wasm.capabilities());
      } catch (error) {
        return rethrow(error);
      }
    },

    /** The four statuses, read out of the Rust enum rather than restated here. */
    statuses(): VerificationStatus[] {
      return wasm.statuses() as VerificationStatus[];
    },

    version(): string {
      return wasm.version();
    },
  };
}

export type AudioProvenanceClient = ReturnType<typeof createClient>;
