export * from "./types.js";

/** Everything `verify` accepts. A `File` is a `Blob`, so browsers are covered by that arm. */
export type AudioInput = string | URL | Uint8Array | ArrayBuffer | ArrayBufferView | Blob;
