import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  AudioProvenanceError,
  capabilities,
  sign,
  verify,
  webCryptoSigner,
} from "@writerslogic/audio-provenance-sdk";

const fixture = (name) => fileURLToPath(new URL(`./fixtures/${name}`, import.meta.url));
const json = async (name) => JSON.parse(await readFile(fixture(name), "utf8"));

const flatStore = await json("trust-store.json");
const unchecked = await json("trust-store-v1-unchecked.json");
const checked = await json("trust-store-v1-checked.json");
const revoked = await json("trust-store-v1-revoked.json");
// Pinned so the fixtures' validity windows cannot lapse under the test.
const trustEvaluatedAt = "2027-01-01T00:00:00Z";
const codes = (result) => result.findings.map((finding) => finding.code);

test("a chained store with no list from the anchor is revocation_unchecked, never silently ok", async () => {
  const result = await verify(fixture("embedded.wav"), { trustStore: unchecked, trustEvaluatedAt });
  assert.equal(result.status, "verified");
  assert.equal(result.identity, "Signal Room Studios");
  assert.equal(result.revocationStatus, "revocation_unchecked");
  assert.ok(codes(result).includes("revocation_unchecked"));
});

test("a flat v0 store carries no signed list, so it is revocation_unchecked too", async () => {
  const result = await verify(fixture("embedded.wav"), { trustStore: flatStore });
  assert.equal(result.identity, "Signal Room Studios");
  assert.equal(result.revocationStatus, "revocation_unchecked");
});

test("a signed list from the anchor that omits the key is checked_not_revoked", async () => {
  const result = await verify(fixture("embedded.wav"), { trustStore: checked, trustEvaluatedAt });
  assert.equal(result.status, "verified");
  assert.equal(result.identity, "Signal Room Studios");
  assert.equal(result.revocationStatus, "checked_not_revoked");
  assert.equal(codes(result).includes("revocation_unchecked"), false);
});

test("a revoked key fails closed and loses its name", async () => {
  const result = await verify(fixture("embedded.wav"), { trustStore: revoked, trustEvaluatedAt });
  assert.equal(result.status, "untrusted");
  assert.equal(result.identity, null);
  assert.equal(result.identityProofLevel, "unknown_unobserved");
  assert.equal(result.revocationStatus, "revoked");
  const rejection = result.findings.find((finding) => finding.code === "trust_anchor_rejected");
  assert.match(rejection.message, /^signer_revoked/);
});

test("without a trust store there is no identity and so nothing to revoke", async () => {
  const result = await verify(fixture("embedded.wav"));
  assert.equal(result.revocationStatus, "not_applicable");
});

test("a chained store is judged at the pinned instant, so a lapsed window refuses the name", async () => {
  const result = await verify(fixture("embedded.wav"), {
    trustStore: checked,
    trustEvaluatedAt: "2200-01-01T00:00:00Z",
  });
  assert.equal(result.identity, null);
  assert.equal(result.status, "untrusted");
});

async function webCryptoKey() {
  const pair = await globalThis.crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
  const publicKey = new Uint8Array(await globalThis.crypto.subtle.exportKey("raw", pair.publicKey));
  return { pair, publicKey };
}

test("capabilities advertise external signing and still no in-package signing", () => {
  const { platform } = capabilities();
  assert.equal(platform.signing, false);
  assert.equal(platform.externalSigning, true);
});

test("a WebCrypto-signed record verifies under its own key and names nobody", async () => {
  const { pair, publicKey } = await webCryptoKey();
  const signer = webCryptoSigner(pair.privateKey, publicKey);
  const signed = await sign(fixture("master.wav"), signer, { signedAt: "2026-09-01T00:00:00Z" });
  assert.match(signed.locator, /^[0-9a-f]{12}$/);
  assert.equal(signed.signerId.length, 16);

  const record = JSON.parse(new TextDecoder().decode(signed.record));
  assert.equal(record.portable_signature.algorithm, "Ed25519-SHA256");
  assert.equal(record.portable_signature.signer_identity, "not_established");
  assert.equal(record.portable_signature.signer_identity_proof_level, "unknown_unobserved");

  const result = await verify(fixture("master.wav"), { records: [signed.registryRecord] });
  assert.equal(result.method, "content_hash_lookup");
  assert.equal(result.signature.valid, true);
  assert.equal(result.identity, null);
  assert.equal(result.identityProofLevel, "unknown_unobserved");
  assert.equal(result.match, 1);
});

test("a signature made under a different key is refused before any record is returned", async () => {
  const claimed = await webCryptoKey();
  const actual = await webCryptoKey();
  const signer = {
    publicKeyHex: webCryptoSigner(claimed.pair.privateKey, claimed.publicKey).publicKeyHex,
    sign: async (digest) => new Uint8Array(await globalThis.crypto.subtle.sign("Ed25519", actual.pair.privateKey, digest)),
  };
  await assert.rejects(
    () => sign(fixture("master.wav"), signer),
    (error) => error instanceof AudioProvenanceError,
  );
});

test("a signer that returns the wrong length, or throws, is refused with a named code", async () => {
  const { publicKey } = await webCryptoKey();
  const publicKeyHex = webCryptoSigner({}, publicKey).publicKeyHex;
  await assert.rejects(
    () => sign(fixture("master.wav"), { publicKeyHex, sign: () => new Uint8Array(63) }),
    (error) => error.code === "signer_output_invalid",
  );
  await assert.rejects(
    () =>
      sign(fixture("master.wav"), {
        publicKeyHex,
        sign: () => {
          throw new Error("hsm offline");
        },
      }),
    (error) => error.code === "signer_failed",
  );
});

test("a malformed public key is refused and no private key is accepted anywhere", async () => {
  await assert.rejects(
    () => sign(fixture("master.wav"), { publicKeyHex: "AB".repeat(32), sign: () => new Uint8Array(64) }),
    (error) => error.code === "option_invalid",
  );
  await assert.rejects(
    () => sign(fixture("master.wav"), { publicKeyHex: "ab".repeat(32), sign: () => new Uint8Array(64) }, { privateKey: "x" }),
    (error) => error instanceof AudioProvenanceError,
  );
});

test("a manifest altered between prepare and seal no longer verifies under the signature", async () => {
  const require = createRequire(import.meta.url);
  const wasm = require("../wasm/node/audio_provenance_wasm.js");
  const { pair, publicKey } = await webCryptoKey();
  const publicKeyHex = webCryptoSigner(pair.privateKey, publicKey).publicKeyHex;
  const bytes = new Uint8Array(await readFile(fixture("master.wav")));
  const prepared = JSON.parse(
    wasm.prepareSigning(bytes, JSON.stringify({ publicKeyHex, signedAt: "2026-09-01T00:00:00Z" })),
  );
  const digest = Uint8Array.from(prepared.digestHex.match(/../g), (pair) => Number.parseInt(pair, 16));
  const signature = Buffer.from(await globalThis.crypto.subtle.sign("Ed25519", pair.privateKey, digest)).toString("hex");
  const tampered = prepared.unsignedManifest.replace("2026-09-01", "2026-09-02");
  assert.notEqual(tampered, prepared.unsignedManifest);
  assert.throws(() => wasm.sealManifest(tampered, publicKeyHex, signature));
  assert.ok(wasm.sealManifest(prepared.unsignedManifest, publicKeyHex, signature).length > 0);
});
