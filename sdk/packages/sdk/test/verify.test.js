import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  AudioProvenanceError,
  capabilities,
  inspect,
  locators,
  statuses,
  verify,
} from "@writerslogic/audio-provenance-sdk";

const fixture = (name) => fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url));
const json = async (name) => JSON.parse(await readFile(fixture(name), "utf8"));

const trustStore = await json("trust-store.json");
const record = await json("registry-record.json");
const nullTest = await json("null-test.json");
const registry = { records: [record], registryName: "demo", nullTest };

test("the status union is exactly the four the Rust enum defines", () => {
  assert.deepEqual(statuses(), ["verified", "changed", "untrusted", "not_found"]);
});

test("capabilities states the wasm platform limits rather than the native ones", () => {
  const caps = capabilities();
  assert.equal(caps.acousticRerecording, "unsupported");
  assert.deepEqual(caps.registryBackends, ["caller_supplied_records"]);
  assert.equal(caps.platform.filesystem, false);
  assert.equal(caps.platform.network, false);
  assert.equal(caps.platform.sidecarManifest, false);
  assert.equal(caps.softBindingVerifiedRequiresNullTest, true);
});

test("an embedded manifest verifies from the bytes alone, with no registry", async () => {
  const result = await verify(fixture("embedded.wav"), { trustStore });
  assert.equal(result.status, "verified");
  assert.equal(result.method, "embedded_manifest");
  assert.equal(result.matchBasis, "hard_exact");
  assert.equal(result.match, 1);
  assert.equal(result.identity, "Signal Room Studios");
  assert.equal(result.identityProofLevel, "externally_verified");
  assert.match(result.signedAt, /^\d{4}-\d{2}-\d{2}$/);
  assert.equal(result.registry, null);
  assert.equal(result.markRecovery.recovered, false);
  assert.equal(result.recordingAssociation.status, "exact");
  assert.equal(result.recordingAssociation.established, true);
});

test("without a trust store the same file verifies and names nobody", async () => {
  const result = await verify(fixture("embedded.wav"));
  assert.equal(result.status, "untrusted");
  // The signature is cryptographically perfect. Key possession is not identity, and a self-asserted
  // name printed as a verified one would be worse than no name at all.
  assert.equal(result.signature.valid, true);
  assert.equal(result.identity, null);
  assert.equal(result.identityProofLevel, "unknown_unobserved");
});

test("caller-supplied records are a real registry, and the result says which one answered", async () => {
  const result = await verify(fixture("signed.wav"), { trustStore, ...registry });
  assert.equal(result.status, "verified");
  assert.equal(result.method, "content_hash_lookup");
  assert.equal(result.registry.kind, "memory");
  assert.equal(result.registry.name, "demo");
});

test("a transcode loses the hard binding and is carried by the soft one", async () => {
  const result = await verify(fixture("transcoded.mp3"), { trustStore, ...registry });
  assert.equal(result.status, "verified");
  assert.equal(result.matchBasis, "mark_and_fingerprint");
  assert.equal(result.binding.proofLevel, "inferred");
  assert.equal(result.markRecovery.recovered, true);
  assert.equal(result.markRecovery.located, true);
  assert.equal(result.markRecovery.recordLocatorMatched, true);
  assert.equal(result.recordingAssociation.status, "associated");
  assert.equal(result.recordingAssociation.established, true);
  // Every soft basis is clamped below 1, which makes `match === 1` an exact test for exact bytes.
  assert.ok(result.match < 1);
  assert.equal(typeof result.binding.falsePositiveRateAtMatch, "number");
});

test("without a null-test report no soft binding may claim verified", async () => {
  const result = await verify(fixture("transcoded.mp3"), {
    trustStore,
    records: [record],
    registryName: "demo",
  });
  assert.equal(result.status, "untrusted");
  assert.equal(result.reason, "soft_binding_false_positive_rate_unknown");
  assert.equal(result.binding.falsePositiveRateAtMatch, null);
});

test("an unregistered file is not_found, and match is null rather than a measured zero", async () => {
  const result = await verify(fixture("master.wav"), { trustStore, ...registry });
  assert.equal(result.status, "not_found");
  assert.equal(result.identity, null);
  assert.equal(result.signedAt, null);
  assert.equal(result.matchBasis, "none");
  assert.equal(result.match, null);
  assert.equal(result.binding.match, 0);
});

test("verify accepts a path, a Uint8Array and a File alike", async () => {
  const bytes = new Uint8Array(await readFile(fixture("embedded.wav")));
  const fromPath = await verify(fixture("embedded.wav"), { trustStore });
  const fromBytes = await verify(bytes, { trustStore });
  const fromFile = await verify(new File([bytes], "embedded.wav"), { trustStore });
  assert.equal(fromPath.contentSha256, fromBytes.contentSha256);
  assert.equal(fromPath.contentSha256, fromFile.contentSha256);
  assert.equal(fromFile.status, "verified");
});

test("a locator is recoverable before any record is in hand", async () => {
  const found = await locators(fixture("transcoded.mp3"));
  assert.match(found.contentSha256, /^[0-9a-f]{64}$/);
  assert.equal(found.locators.length, 1);
  assert.match(found.locators[0].locatorHex, /^[0-9a-f]{12}$/);
  assert.equal(found.locators[0].confidenceClass, "strong");
});

test("a misspelled option is refused, with the Rust side's own error code", async () => {
  await assert.rejects(
    () => verify(fixture("embedded.wav"), { trustStoer: trustStore }),
    (error) => error instanceof AudioProvenanceError && error.code === "option_invalid",
  );
});

test("inspect reports the container and the ladder, and no verdict", async () => {
  const report = await inspect(fixture("embedded.wav"), { trustStore });
  assert.equal(report.container, "wav");
  assert.equal(report.sampleRate, 48000);
  assert.equal(report.channels, 2);
  assert.ok(Math.abs(report.durationSeconds - 20) < 0.05);
  assert.equal(report.manifestRecovered, true);
  assert.equal(report.manifestSchema, "audio-provenance-manifest-v1");
  assert.equal(report.method, "embedded_manifest");
  assert.equal(report.trace.length, 6);
  assert.equal(report.incomplete, false);
  assert.equal("status" in report, false);
});
