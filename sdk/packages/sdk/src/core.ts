import type {
  AudioInput,
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
     * The GenoMark locators recoverable from this audio.
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
