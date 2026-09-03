const MAX_REQUEST_BYTES = 512;
const DIGEST_HEX_LENGTH = 64;
const ALGORITHM = "Ed25519-SHA256";

export interface SignerEnv {
  SIGNING_KEY_PKCS8: string;
  SERVICE_TOKEN: string;
}

let privateKeyPromise: Promise<CryptoKey> | undefined;

export default {
  async fetch(request: Request, env: SignerEnv): Promise<Response> {
    const url = new URL(request.url);
    if (request.method !== "POST" || url.pathname !== "/sign") {
      return text("Not Found", 404);
    }
    if (!request.headers.get("content-type")?.toLowerCase().startsWith("application/json")) {
      return text("Content-Type must be application/json", 415);
    }
    if (!authenticated(request, env.SERVICE_TOKEN)) {
      return text("Unauthorized", 401);
    }

    try {
      const payload = await readRequest(request);
      if (!isDigest(payload.digest)) {
        return text('Missing or invalid "digest" field', 400);
      }
      const key = await privateKey(env.SIGNING_KEY_PKCS8);
      const signature = await crypto.subtle.sign(
        { name: "Ed25519" },
        key,
        hexToBytes(payload.digest),
      );
      return Response.json(
        {
          algorithm: ALGORITHM,
          signature: bytesToHex(new Uint8Array(signature)),
        },
        { headers: securityHeaders() },
      );
    } catch (error) {
      if (error instanceof RequestError) {
        return text(error.message, error.status);
      }
      // Never interpolate an exception here: engine errors can include input material.
      console.error("remote signing operation failed");
      return text("Signing failed", 500);
    }
  },
} satisfies ExportedHandler<SignerEnv>;

function authenticated(request: Request, expectedToken: string): boolean {
  const supplied = request.headers.get("authorization");
  if (supplied === null || expectedToken.length === 0) return false;
  const encoder = new TextEncoder();
  const actual = encoder.encode(supplied);
  const expected = encoder.encode(`Bearer ${expectedToken}`);
  return actual.byteLength === expected.byteLength && crypto.subtle.timingSafeEqual(actual, expected);
}

async function readRequest(request: Request): Promise<{ digest?: unknown }> {
  const declared = Number(request.headers.get("content-length") ?? 0);
  if (Number.isFinite(declared) && declared > MAX_REQUEST_BYTES) {
    throw new RequestError(413, "Request body too large");
  }
  if (request.body === null) throw new RequestError(400, "Request body is required");
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > MAX_REQUEST_BYTES) {
      await reader.cancel();
      throw new RequestError(413, "Request body too large");
    }
    chunks.push(value);
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    const value: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes));
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new RequestError(400, "Request body must be a JSON object");
    }
    const keys = Object.keys(value);
    if (keys.length !== 1 || keys[0] !== "digest") {
      throw new RequestError(400, 'Only the "digest" field is accepted');
    }
    return value as { digest?: unknown };
  } catch (error) {
    if (error instanceof RequestError) throw error;
    throw new RequestError(400, "Request body is not valid JSON");
  }
}

function privateKey(encodedPkcs8: string): Promise<CryptoKey> {
  privateKeyPromise ??= importPrivateKey(encodedPkcs8);
  return privateKeyPromise;
}

async function importPrivateKey(encodedPkcs8: string): Promise<CryptoKey> {
  const raw = decodeBase64(encodedPkcs8);
  try {
    return await crypto.subtle.importKey(
      "pkcs8",
      raw,
      { name: "Ed25519" },
      false,
      ["sign"],
    );
  } finally {
    // The binding is necessarily visible as a string in this isolate; erase the decoded copy as
    // soon as Web Crypto has created the non-extractable key handle.
    raw.fill(0);
  }
}

function decodeBase64(value: string): Uint8Array {
  if (value.length === 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) {
    throw new Error("invalid key encoding");
  }
  const decoded = atob(value);
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

function isDigest(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function hexToBytes(value: string): Uint8Array {
  const bytes = new Uint8Array(DIGEST_HEX_LENGTH / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(value.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

function bytesToHex(value: Uint8Array): string {
  return Array.from(value, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function securityHeaders(): HeadersInit {
  return {
    "cache-control": "no-store",
    "content-type": "application/json; charset=UTF-8",
    "x-content-type-options": "nosniff",
  };
}

function text(body: string, status: number): Response {
  return new Response(body, { status, headers: { ...securityHeaders(), "content-type": "text/plain; charset=UTF-8" } });
}

class RequestError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}
