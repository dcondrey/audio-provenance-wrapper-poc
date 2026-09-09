# audio-provenance-sdk

Audio provenance: a signed manifest bound to a recording, an inaudible watermark that finds the
manifest again after the bytes change, and a verifier that says one of four words and means it.

The claim this system makes is narrow on purpose. `verified` means a signature over a hash chain
checked out AND a trust anchor you configured vouches for the signer. It never means "this is
authentic" and a missing mark never means "this is synthetic".

```
$ audio-provenance verify signed.wav --registry local --trust-store store.json --null-test null.json
verified   signed.wav
  identity   Signal Room Studios        (externally_verified via Signal Room Studios CA [anchor signal-room-ca key 6359d572e44b4fb1 depth 1])
  signed     2026-09-01
  match      1.00                       (hard binding, exact)
  registry   local (filesystem)         recovered via sidecar_manifest
```

Read `STATUS.md` before quoting a number from this repository. It separates what was observed by
running the commands from what was reported by someone else, and it lists what is open.

## Build and test

Stable 1.96.0, pinned in `rust-toolchain.toml`. Edition 2024, resolver 3.

```sh
cargo build --workspace --all-targets
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --exclude apw-watermark-neural -- -D warnings
cargo clippy -p apw-watermark-neural --all-targets -- -D warnings
```

`apw-watermark-neural` is excluded from the `--all-features` line deliberately, not to hide a warning. Its
`download-binaries` and `load-dynamic` features are two mutually exclusive ways to obtain the ONNX
Runtime and the crate raises a `compile_error!` when both are on, so `--all-features` is not a valid
configuration for it. Clippy it on its own line with its own default features.

Some tests take minutes; two suites are `#[ignore]`d because they need a tool or a corpus that is
not in the repository. `STATUS.md` names all six and what each needs.

## Reproduce the identity demo

```sh
cargo build --release -p audio-provenance-cli
./demo.sh                       # or ./demo.sh /some/workdir
```

`demo.sh` runs eight steps live and prints every command before it runs it: an exact verification
that resolves a real name, the same file after a 128k mp3 transcode still verifying through the
watermark and the signed reference constellation, that soft binding refusing to verify without a
measured false-positive rate, the same file with no trust store, the same file after the signer is
revoked, the mark lifted onto unrelated audio, an unregistered file reported neutrally, and the
sparse-master case where the transcode does NOT verify and the CLI says exactly why.

It uses a real, licence-verified recording if you have fetched the corpus (see below) and
synthesised pink noise if you have not; it says which one it used. **The two are not equivalent.**
Pink noise verifies through the mp3 step where most real music does not; `STATUS.md` has the
measurement.

### By hand

```sh
G=target/release/audio-provenance
export AUDIO_PROVENANCE_REGISTRY_ROOT=$PWD/reg

$G keygen --out signer.key
$G keygen --out authority.key
$G registry init ./reg

# An authority is self-signed. It is trusted because a verifier is handed it,
# not because it asserts anything about itself.
$G trust init-authority --key authority.key --anchor-id signal-room-ca \
  --name "Signal Room Studios CA" --out anchor.json
$G trust issue --anchor anchor.json --issuer-key authority.key \
  --subject-key signer.pub --name "Signal Room Studios" --record-id sr-001 --out record.json
$G trust export-anchor --anchor anchor.json --out store.json   # holds no private key
$G trust add --store store.json --document record.json

$G sign master.wav --key signer.key --mark --out signed.wav --registry local
$G verify signed.wav --registry local --trust-store store.json --null-test <report.json>
```

Drop `--trust-store` and the identity reads `not_established`: a valid signature proves key
possession, never identity. Revoke the signer and it reads `not_established` again, but the output
also prints the revocation, retroactively and by name.

Exit codes: `0` verified, `1` changed, `2` untrusted, `3` not_found, `4` incomplete (a rung could
not run, which is an operational fault and not a verdict), `64` usage, `65` unreadable input.

## The corpus

No audio is committed. `corpus/fetch.py` reproduces both corpora and writes a manifest naming each
item's source URL, its licence exactly as the source publishes it, and the SHA-256 of the WAV the
bench reads. An item whose source publishes no explicit licence is not downloaded.

```sh
uv run --no-project python corpus/fetch.py characterisation      # 13 items
uv run --no-project python corpus/fetch.py null --count 1000     # 1,000 distinct works
```

The first run downloads about 7.5 GB into `corpus/_download/` and keeps it. `corpus/README.md` has
the selection rule and the sources that were rejected and why.

## Benches

```sh
# Robustness and perceptual bench over real music beside the synthetic recipes, one report
cargo run --release -p audio-provenance-nulltest --bin audio-provenance-corpus-bench -- \
  --real-dir corpus/characterisation --out-dir bench-out/real-corpus

# Was the real-vs-synthetic difference real, or did the harness move underneath?
uv run --no-project python corpus/analyse-real-vs-synthetic.py \
  bench-out/real-corpus/apw-watermark-lepqim-v1-real-corpus.json \
  --baseline bench-out/final2/apw-watermark-lepqim-v1.json

# The null test: how often does the detector accept a payload out of never-marked audio
cargo run --release -p audio-provenance-nulltest --bin audio-provenance-null-test -- \
  --corpus-dir corpus/null --out-dir bench-out/null

# A motivated adversary who has read the spec
cargo run --release -p audio-provenance-attack -- campaign \
  --corpus-dir corpus/characterisation --out bench-out/attacks/report.json

# Build the sibling programs once, then run all four arms concurrently and aggregate their reports
cargo build --release -p audio-provenance-nulltest --bins \
  -p audio-provenance-attack --bin audio-provenance-attack
cargo run --release -p audio-provenance-nulltest --bin audio-provenance-qualify -- \
  --null-dir corpus/null --corpus-dir corpus/characterisation \
  --out-dir qualification/run --report qualification/run/baseline.json \
  --null-manifest corpus/MANIFEST-null.json \
  --corpus-manifest qualification/watermark-adversarial-v1.json
```

The checked-in baseline is `qualification/reports/classical-apw-watermark/baseline.md`. Its production
runner has the default `classical-apw-watermark` feature and no `apw-watermark-neural` dependency; a permanent
test rejects any accidental link to the experimental crate.

## Remote key custody and registry publication

`audio_provenance_sdk::ManifestSigner` abstracts local and remote signing.
`CloudflareSigner` sends the SHA-256 digest of canonical manifest bytes to the Worker in
`workers/hsm-signer`, pins and locally verifies the returned public-key signature, disables
redirects, bounds responses, and supports bearer or Cloudflare Access service-token headers.
`HttpRegistryBackend::new_authenticated` supplies the production writable registry backend using
idempotent authenticated record `PUT`s.

Remote custody does not establish identity. Configure a trust anchor for the pinned public key if
a verifier should report a creator or studio name.

A soft binding cannot reach `verified` without a null-test report passed to `--null-test`. That gate
is in the type system, not in a review note: the false-positive rate behind an inference is measured
or the inference does not get to be a verdict.

## Layout

| Crate | Role |
| --- | --- |
| `audio-provenance-core` | `apw-json-sort-v1` canonical JSON, Ed25519, the provenance vocabulary, the hash chain |
| `audio-provenance-audio` | Decode, WAV encode, FFT/STFT/MDCT, resampling, filters, metrics |
| `audio-provenance-registry` | Signed-manifest registry: a trait, a local backend and an HTTP one |
| `audio-provenance-bench` | Channel simulators, the measurement runner, the null-test statistics |
| `apw_watermark` | Watermark-Q (`apw-watermark-lepqim-v1`): 56-bit payload, blind CRC-gated detection |
| `audio-provenance-manifest` | The signed record, and the POC `audio-provenance-manifest-v0` interop path |
| `apw_trace` | The recovery ladder, the soft binding, and the status mapping |
| `audio-provenance-trust` | Anchors, signer records, revocation, chain evaluation: key possession into a name |
| `audio-provenance-c2pa` | C2PA claim-structure reading, checked against `c2patool` rather than our own writer |
| `audio-provenance-sdk` | The public facade: verify, inspect, sign, embed, publish, `capabilities()` |
| `audio-provenance-cli` | The `audio-provenance` binary. Rendering and exit codes; no provenance logic |
| `audio-provenance-wasm` | The wasm-bindgen surface, published as `@writerslogic/audio-provenance-sdk` |
| `audio-provenance-nulltest` | The null-test and real-corpus bench runners |
| `audio-provenance-attack` | The adversarial campaign against the watermark |
| `apw-watermark-neural` | Watermark-N, the v2 learned acoustic mark. Research, not shipped |

`members` is `["crates/*"]` rather than a list of names, because cargo refuses to load the entire
workspace when a named member's directory is missing.

## The interop contract

`audio-provenance-core` must produce byte-identical signing input to the Python audio-provenance POC, whose
canonicalisation is:

```python
json.dumps(value, sort_keys=True, separators=(",", ":"),
           ensure_ascii=False, allow_nan=False).encode("utf-8")
```

That is `apw-json-sort-v1`. `audio_provenance_core::canonical_json` reimplements it, including CPython's
float `repr` rules, and the proof is in `crates/audio-provenance-core/tests/`: fixtures generated by the
POC's own interpreter, plus two real signed manifests written by the POC daemon that this crate's
verifier accepts. Regenerate the fixtures with:

```sh
/Volumes/A/audio-provenance/.venv/bin/python \
  crates/audio-provenance-core/tests/fixtures/generate.py
```

`audio-provenance-core` is `no_std` + `alloc` with `default = ["std"]`. The `std` feature adds exactly one
thing: reading a signing key off disk. Nothing else in the crate touches the filesystem, the
network, a process, or the clock; timestamps are parameters, never `SystemTime::now()`.

## Honest limits

- **Acoustic re-recording is unsupported in v1.** `capabilities()` returns the literal
  `"unsupported"`. The bench's acoustic rows are a *simulated* path and are labelled as such.
- **The watermark is a locator, not an authenticator.** Against an attacker who has read
  `docs/WATERMARK_SPEC.md` it has no removal and no forgery resistance; `docs/WATERMARK_ATTACKS.md`
  measures exactly how much. Verdicts rest on the signature over the hash chain, never on the mark.
- **Perceptual figures are segmental SNR and a noise-to-mask ratio.** Neither is PEAQ and neither is
  a listening test.
- `docs/THREAT_MODEL.md` lists the findings from a security audit, including the ones not fixed.
