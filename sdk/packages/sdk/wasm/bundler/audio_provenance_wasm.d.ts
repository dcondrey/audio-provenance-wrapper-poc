/* tslint:disable */
/* eslint-disable */

/**
 * What this build can actually do, with the wasm platform limits stated rather than implied.
 */
export function capabilities(): string;

/**
 * Reports what the recovery ladder found, with no verdict.
 *
 * The native `inspect` takes a path so it can run the sidecar rung; there is no directory here, so
 * this reaches the ladder through `verify_bytes` and drops the verdict on the way out. One
 * consequence is inherited and worth stating: a search that could not finish RAISES here, where
 * the native `inspect` would have reported it. With caller-supplied records that needs a supplied
 * record set to be internally inconsistent, which is a caller error either way.
 */
export function inspectBytes(audio: Uint8Array, options_json?: string | null): string;

/**
 * The Watermark locators recoverable from this audio, for a caller that must fetch records before
 * it can supply them.
 *
 * At most one: blind detection reports the single payload the CRC accepted, and `locators` is
 * empty when nothing decoded. A locator is an INDEX, not an identity: several records can share
 * one, and recovering one proves nothing on its own. Only [`verify_bytes`] produces a verdict.
 */
export function locators(audio: Uint8Array): string;

/**
 * Turns a wasm trap into a readable JS stack trace. Without it a panic anywhere in the graph
 * reaches the caller as `RuntimeError: unreachable` and nothing else.
 */
export function start(): void;

/**
 * The four statuses, so a JS test can assert the union has not silently grown.
 */
export function statuses(): string[];

/**
 * Verifies audio already in memory.
 *
 * `options_json` is the JSON text of the options object; `null` or omitted means defaults. The
 * return is the JSON text of a `VerifyResult` with every key in camelCase.
 */
export function verifyBytes(audio: Uint8Array, options_json?: string | null): string;

export function version(): string;
