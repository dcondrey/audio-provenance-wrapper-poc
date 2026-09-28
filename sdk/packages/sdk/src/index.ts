/**
 * `@writerslogic/audio-provenance-sdk` — audio provenance you can verify from the file alone.
 *
 * ```ts
 * import { verify } from "@writerslogic/audio-provenance-sdk";
 * const result = await verify(audioFile);
 * result.status;   // "verified" | "changed" | "untrusted" | "not_found"
 * result.identity; // "Signal Room Studios"
 * result.signedAt; // "2026-03-14"
 * result.match;    // 1.0
 * ```
 *
 * `identity` is `null` unless `options.trustStore` anchors the signing key to a name, and that is
 * the correct answer, not a gap: a valid self-generated signature proves key possession and says
 * nothing about who holds the key.
 */

import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";

import { createClient, AudioProvenanceError, type AudioProvenanceWasm } from "./core.js";

export { AudioProvenanceError, webCryptoSigner } from "./core.js";
export type { AudioProvenanceClient, AudioProvenanceWasm, PathReader, Runtime } from "./core.js";
export * from "./public.js";

// wasm-pack's `nodejs` target emits CommonJS that instantiates the module synchronously on load.
// `createRequire` is how an ESM package reaches it without shipping a second build step.
const require = createRequire(import.meta.url);
const wasm = require("../wasm/node/audio_provenance_wasm.js") as AudioProvenanceWasm;

const client = createClient({
  wasm,
  readPath: async (source) => new Uint8Array(await readFile(source)),
});

export const verify = client.verify;
export const inspect = client.inspect;
export const sign = client.sign;
export const locators = client.locators;
export const capabilities = client.capabilities;
export const statuses = client.statuses;
export const version = client.version;

export default { verify, inspect, sign, locators, capabilities, statuses, version, AudioProvenanceError };
