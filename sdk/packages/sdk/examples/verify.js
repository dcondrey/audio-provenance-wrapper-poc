// The README snippet, run against a real signed file. `node examples/verify.js [file]`
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

import { verify } from "@writerslogic/audio-provenance-sdk";

const here = (relative) => fileURLToPath(new URL(relative, import.meta.url));
const audioFile = process.argv[2] ?? here("../test/fixtures/embedded.wav");
const trustStore = JSON.parse(await readFile(here("../test/fixtures/trust-store.json"), "utf8"));

const result = await verify(audioFile, { trustStore });

console.log(result.status); //   "verified" | "changed" | "untrusted" | "not_found"
console.log(result.identity); // "Signal Room Studios"
console.log(result.signedAt); // "2026-03-14"
console.log(result.match); //    1.0

console.log("\nthe whole result object:");
console.dir(result, { depth: null });
