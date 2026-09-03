import { describe, expect, it } from "vitest";
import worker, { type SignerEnv } from "../src/index";

const TOKEN = "test-service-token-that-is-not-used-in-production";

async function testEnv(): Promise<{ env: SignerEnv; publicKey: CryptoKey }> {
  const keys = (await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const pkcs8 = new Uint8Array((await crypto.subtle.exportKey("pkcs8", keys.privateKey)) as ArrayBuffer);
  let binary = "";
  for (const byte of pkcs8) binary += String.fromCharCode(byte);
  return {
    env: { SIGNING_KEY_PKCS8: btoa(binary), SERVICE_TOKEN: TOKEN },
    publicKey: keys.publicKey,
  };
}

function request(body: unknown, token = TOKEN): Request {
  return new Request("https://signer.example/sign", {
    method: "POST",
    headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

describe("HSM proxy Worker", () => {
  it("guards the route and authentication", async () => {
    const { env } = await testEnv();
    expect((await worker.fetch(new Request("https://signer.example/sign"), env)).status).toBe(404);
    expect((await worker.fetch(request({ digest: "ab".repeat(32) }, "wrong"), env)).status).toBe(401);
  });

  it("strictly validates the digest and request shape", async () => {
    const { env } = await testEnv();
    for (const body of [{ digest: "AB".repeat(32) }, { digest: "ab" }, { digest: "ab".repeat(32), extra: true }]) {
      expect((await worker.fetch(request(body), env)).status).toBe(400);
    }
  });

  it("returns a verifiable Ed25519 signature over the digest bytes", async () => {
    const { env, publicKey } = await testEnv();
    const digest = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const response = await worker.fetch(request({ digest }), env);
    expect(response.status).toBe(200);
    expect(response.headers.get("cache-control")).toBe("no-store");
    const result = (await response.json()) as { algorithm: string; signature: string };
    expect(result.algorithm).toBe("Ed25519-SHA256");
    expect(result.signature).toMatch(/^[0-9a-f]{128}$/);
    const signature = Uint8Array.from(result.signature.match(/../g) ?? [], (byte) => Number.parseInt(byte, 16));
    const digestBytes = Uint8Array.from(digest.match(/../g) ?? [], (byte) => Number.parseInt(byte, 16));
    expect(await crypto.subtle.verify({ name: "Ed25519" }, publicKey, signature, digestBytes)).toBe(true);
  });

  it("bounds request bodies", async () => {
    const { env } = await testEnv();
    const response = await worker.fetch(request({ digest: "ab".repeat(32), padding: "x".repeat(600) }), env);
    expect(response.status).toBe(413);
  });
});
