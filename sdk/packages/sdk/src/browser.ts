/**
 * The browser and bundler entry point.
 *
 * Identical to the Node entry except that there is no filesystem: `verify("path.wav")` raises
 * `input_unreadable` rather than pretending to read one. Pass the `File` from an `<input>`, a
 * `Blob`, an `ArrayBuffer`, or a `Uint8Array`.
 *
 * Your bundler must be able to load `.wasm`; every current bundler can. Reach this entry through
 * the package's `browser` export condition, or import it directly:
 * `import { verify } from "@writerslogic/audio-provenance-sdk/browser"`.
 */

import * as wasm from "../wasm/bundler/audio_provenance_wasm.js";

import { createClient, type AudioProvenanceWasm } from "./core.js";

export { AudioProvenanceError, webCryptoSigner } from "./core.js";
export type { AudioProvenanceClient, AudioProvenanceWasm, PathReader, Runtime } from "./core.js";
export * from "./public.js";

const client = createClient({ wasm: wasm as unknown as AudioProvenanceWasm });

export const verify = client.verify;
export const inspect = client.inspect;
export const sign = client.sign;
export const locators = client.locators;
export const capabilities = client.capabilities;
export const statuses = client.statuses;
export const version = client.version;
