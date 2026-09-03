# Cloudflare remote key-custody signer

This Worker accepts a 32-byte SHA-256 digest at `POST /sign` and returns an
`Ed25519-SHA256` signature. Requests require the `SERVICE_TOKEN` bearer secret. A deployment may
add Cloudflare Access service-token or mTLS policy in front of that application-layer check.

Set and deploy the secrets without putting either value in `wrangler.jsonc`:

```sh
npx wrangler secret put SERVICE_TOKEN
npx wrangler secret put SIGNING_KEY_PKCS8
npm run deploy
```

`SIGNING_KEY_PKCS8` is base64-encoded Ed25519 PKCS#8. The Worker imports it with
`extractable: false`, caches only the resulting `CryptoKey`, and clears its decoded byte buffer.
Cloudflare Secrets encrypt and hide the binding outside the runtime. This is a remote
key-custody proxy, not a hardware HSM: the secret binding is still a JavaScript string inside the
isolate during import, and `extractable: false` prevents exporting the imported `CryptoKey` but
cannot make the original environment string disappear. Use a managed HSM/KMS integration when a
hardware-backed non-exportability claim is required.

The Rust client pins the expected public key, rejects any other algorithm, verifies every returned
signature locally, disables redirects, bounds responses, and redacts credentials from `Debug`.
