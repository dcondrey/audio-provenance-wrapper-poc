# Status

Written 2026-08-31 from an integration gate run on this tree. Every number below is either
**OBSERVED** (a command was run in this session and its output read) or **REPORTED** (it came from a
document or an earlier effort and was not re-derived here). Nothing is stated as measured that was
not measured.

Update 2026-09-01: the interior-substitution defect documented below is now closed structurally by
signed local-region coverage, a 0.75-second maximum unexplained interior gap, duration/block
distribution checks, and offset-discontinuity rejection. Unit tests cover 1%, 2%, 5%, 10%, 25%,
and 50% interior gaps plus head/tail replacement, arbitrary crops, reordering, and duplication; an
actual-audio middle-substitution regression rejects while an arbitrary crop associates. The fast
suite is not a substitute for the predeclared long null/adversarial corpus, which remains a nightly
qualification gate. Mark recovery and recording association are now separate result fields, and a
located record with rejected association produces `changed`.

Qualification update 2026-09-01: the full 1,000-item null artifact, all 13 characterization items,
585 recording-association trials, and a fresh 591-row/all-13-item adversarial campaign are
aggregated in `qualification/reports/classical-apw-watermark/baseline.json`. **OBSERVED:** 0/37,000 null
false accepts (rule-of-three 95% upper bound 0.003003), 512/740 exact locator recoveries, 268/273
expected associations, and 298/312 material alterations rejected. The run is complete and the
declared targets fail. `apw-watermark-neural` is absent from the production runner's dependency graph.

The tree was being edited by another effort while this gate ran: `crates/apw-trace/src/ladder.rs`
and `crates/apw-trace/tests/soft_mark_locator.rs` changed at 17:10 and 17:13 local, mid-run. Every
result below comes from the tree as it stood after those edits.

## The gate

| Command | Result |
|---|---|
| `cargo build --workspace --all-targets` | **OBSERVED** clean |
| `cargo test --workspace` | **OBSERVED** 217 passed, 0 failed, 6 ignored, over 59 test targets |
| `cargo clippy --workspace --all-targets -- -D warnings` | **OBSERVED** clean |
| `cargo clippy --workspace --exclude apw-watermark-neural --all-targets --all-features -- -D warnings` | **OBSERVED** clean |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | **OBSERVED FAILING**, in `apw-watermark-neural` only. See Open. |
| `cargo fmt --all --check` | **OBSERVED** clean |
| `pnpm --filter @writerslogic/audio-provenance-sdk run build:wasm && build && test` | **OBSERVED** 12 Node tests pass against wasm rebuilt from this tree |

Of the 6 ignored tests, 5 are the `c2patool` differential, ignored because they shell out to a
binary that is not on every machine. `c2patool 0.26.68` **is** on this machine, so they were run:
`cargo test -p audio-provenance-c2pa --test c2patool_differential -- --ignored` → **OBSERVED** 5 passed. The
sixth, `the_affirmation_threshold_meets_its_false_positive_target`, is the reference-constellation
false-positive harness; it was **NOT** run here, so its 812-pair result is REPORTED, not observed.

## Crates

| Crate | Owns |
|---|---|
| `audio-provenance-core` | `apw-json-sort-v1` canonical JSON, Ed25519, the proof-level vocabulary, the hash chain |
| `audio-provenance-audio` | symphonia decode, WAV encode, FFT/STFT/MDCT, rubato resampling, biquads |
| `audio-provenance-registry` | `RegistryBackend`, local + HTTP backends, the name resolver |
| `audio-provenance-bench` | 37 channels, `WatermarkCodec`, the BER / exact-recovery / false-positive runner |
| `apw_watermark` | `apw-watermark-lepqim-v1`, 56-bit payload, blind CRC-gated detection |
| `audio-provenance-manifest` | The signed record model and the POC `audio-provenance-manifest-v0` interop path |
| `audio-provenance-c2pa` | C2PA claim reading and assertion-store integrity, checked against `c2patool` |
| `apw_trace` | The recovery ladder, the container reader **and writer**, the status mapping |
| `audio-provenance-trust` | Anchors, signed signer records, revocation, chain evaluation |
| `audio-provenance-sdk` | `verify` / `sign` / `embed` / `inspect`, `capabilities()` |
| `audio-provenance-cli` | The `audio-provenance` binary |
| `audio-provenance-wasm` | The wasm-bindgen surface, and the caller-supplied registry backend |
| `audio-provenance-attack` | The adversarial campaign against `apw-watermark-lepqim-v1` |
| `audio-provenance-nulltest` | The Watermark null-test and corpus-bench runners |
| `apw-watermark-neural` | Experimental inference path; not a production alternative until it beats classical Watermark on the same blind corpus, latency, memory, false-positive, and perceptual gates |

`members` is `["crates/*"]` rather than a name list, because cargo refuses to load the whole
workspace when a named member's directory is missing.

**Stale claim removed.** An earlier STATUS said the package list had to be spelled out because
`audio-provenance-trust` did not compile. It compiles: `cargo build --workspace --all-targets` built all
fifteen crates. `--workspace` is now correct everywhere except the `--all-features` clippy line.

## Watermark, re-measured this session

Re-run at the baseline's own invocation — same default seed `7450482583528238693`, same payload
`109ac35e0011a7`, same 10-item 30 s corpus, same three real WAVs — in `--release`, and diffed
row by row against `bench-out/final2/apw-watermark-lepqim-v1.txt`. Report:
`bench-out/regate/apw-watermark-lepqim-v1.txt`.

**OBSERVED. The critical arm is unchanged: 0 accepts in 370 unmarked trials, and the per-channel
`fpr` column is 0.000 on all 37 rows.** No false-positive regression.

**OBSERVED. Bit error rate is 0.000 on every channel that returned a payload, on all 37 rows, in
both runs.** No row changed.

**OBSERVED. Eight channels improved their exact-recovery rate. None regressed.**

| channel | exact, baseline → now |
|---|---|
| `aac_256` | 0.700 → 0.800 |
| `aac_64` | 0.400 → 0.500 |
| `requantize_16bit` | 0.700 → 0.800 |
| `crop_2s` | 0.700 → 0.800 |
| `drift_plus_0p1pct` | 0.400 → 0.600 |
| `drift_minus_0p1pct` | 0.400 → 0.600 |
| `acoustic_small_room` (simulated) | 0.300 → 0.600 |
| `chain_transcode_mp3_128_aac_128` | 0.500 → 0.600 |

The other 29 rows are identical. The summary line is identical: `0 pass, 32 fail,
5 recorded-no-threshold, 0 error`. The perceptual table is identical to the last decimal on all ten
items. Mean detection time fell from 519.0 ms to 384.1 ms; that is wall clock on one machine and is
not a quality figure.

**The drift weakness is narrowed, not gone.** An earlier STATUS called ±0.1% playback drift the real
weakness at 0.4 exact recovery. It is 0.6 now, against 0.7 at identity.

**The "0 pass, 32 fail" headline is still a threshold artefact**, unchanged in kind: the pass bar is
0.99 exact recovery over a corpus that includes `synth_near_silence`, which no watermark can carry,
plus two synthetic items measured at −46 and −44 dBFS in the 861–4307 Hz band. Every row's `noPay`
is exactly `1 − exact`; the detector is silent on those three items rather than wrong.

## End to end, run live in a clean directory

A 4-minute 48 kHz stereo master, a fresh signing key, a self-signed authority, a signer record, a
filesystem registry. Full transcript in the report accompanying this run.

| Step | Expected | **OBSERVED** |
|---|---|---|
| sign `--mark`, publish | a record | signed, marked `fa2426c0c3d9`, record published, exit 0 |
| verify the signed WAV | verified, hard, 1.0 | `verified`, `match 1.00 (hard binding, exact)`, identity resolved, exit 0 |
| the same with `--no-sidecar` | the registry answers | `verified`, recovered via `content_hash_lookup`, exit 0 |
| **mp3 128, verify** | **verified via the soft binding** | **`verified (inferred)`, `match 0.99`, Watermark 27/27 blocks + signed constellation, recovered via `apw_watermark_recovery`, exit 0** |
| mp3 128 without `--null-test` | the gate refuses | `untrusted`, exit 2, "a soft binding cannot verify without one" |
| aac 128, verify | verified | `verified (inferred)`, `match 0.99`, 23/27 blocks, exit 0 |
| opus 128, verify | verified | **FAILS**: `input_undecodable`, exit 65. See Open. |
| opus 128 decoded back to WAV | verified | `verified (inferred)`, `match 0.99`, 27/27 blocks, exit 0 |
| replace part of the audio | changed | **PARTLY FAILS**: 8% and 17% replaced still verify. See Open. |
| lift the mark onto unrelated audio | not verified | `changed`, exit 1 |
| an unmarked, unregistered file | not_found, exit 3, neutral | `not_found`, exit 3, "a missing mark is never proof of synthetic origin" |
| a dead HTTP registry | operational, never not_found | exit 4, `(incomplete)`, "an operational fault, not a verdict" |
| the Node SDK on the same files | the CLI's verdicts | `verified`/`hard_exact`/1, `verified`/`mark_and_fingerprint`/0.99, `changed`, `not_found` |

## Fixed during this gate

- **The RIFF provenance writer existed twice and the two copies had opposite semantics.**
  `audio-provenance-cli` replaced an existing `aprv` chunk; `audio-provenance-sdk` appended one. The reader,
  `apw_trace::container::riff_chunk`, returns the FIRST match. So `audio_provenance_sdk::sign` with
  `embed_manifest` over an already-signed asset published a file that verified against the manifest
  it was meant to supersede. **OBSERVED** as a failing test before the fix (recovered `2026-03-14`
  where `2027-05-02` was signed) and passing after. There is now one writer,
  `apw_trace::container::write_riff_chunk`, beside the reader; both crates are adapters over it.
  The consolidation also gave the CLI path the manifest-size bound it lacked, which it could
  previously exceed and write a chunk its own reader would refuse.
- **`capabilities().decodes` advertised `ogg`.** symphonia 0.6.1 ships no Opus decoder and
  `all-codecs` does not include one, so Ogg/Opus does not decode. The claim is now `ogg_vorbis`.
- **A dead registry printed `not_found` as its headline.** Exit 4 already separated it for a script.
  The headline now carries `(incomplete)` beside the filename, the same way `(inferred)` rides
  there, so it is separated for a reader too. The four-word status vocabulary is unchanged.

## Open

- **Interior substitution is closed structurally; the completed scientific qualification fails
  its declared targets.** The
  former predicate used first-to-last aligned span and therefore read 1.0 when matching edges
  surrounded a replaced middle. The current predicate accounts for 0.5-second local regions,
  rejects an unexplained interior gap above 0.75 seconds, requires at least 0.80 local coverage,
  enforces the signed duration/block distribution, and rejects coherent-offset discontinuities.
  Pure tests pin replacement fractions from 1% through 50%, and an actual-audio regression replaces
  the middle four seconds of a 24-second item. The permanent three-axis campaign is declared in
  `qualification/watermark-adversarial-v1.json`: false acceptance, locator recovery, and rejection
  of materially altered signed audio are measured separately. The versioned baseline reports
  19/585 association expectation failures and 273/591 successful adversarial attacks. These are
  production quality gaps, not missing measurements and not reasons to widen the fixed predicates.

- **`cargo clippy --workspace --all-targets --all-features` cannot pass.** `--all-features` enables
  `apw-watermark-neural`'s `download-binaries` and `load-dynamic` together, and that crate raises a
  deliberate `compile_error!` because they are mutually exclusive ONNX Runtime linkage strategies.
  Watermark-N remains experimental and is not a second production path. Everything else is clean
  under `--all-features`; the two clippy lines in the gate table above are the usable form.

- **Opus does not decode.** `.opus` returns `input_undecodable`, exit 65. symphonia 0.6.1 has no
  Opus codec and no feature flag adds one, so this cannot be fixed in a manifest; it needs another
  decoder or an ffmpeg-backed path. The Watermark mark itself survives an opus-128 codec pass
  intact — **OBSERVED** `verified (inferred)`, 27/27 blocks, after decoding the same `.opus` back to
  WAV with ffmpeg — so this is a container/codec support gap, not a robustness one.

- **A trust store the CLI produced cannot be read by the shipped JS SDK.** The CLI has two store
  readers: `audio_provenance_trust::TrustStore` for the chained Ed25519-signed `audio-provenance-trust-store-v1`
  (a file, or a directory with an `anchors/` subdirectory) and `apw_trace::FileTrustStore` for the
  flat asserted `audio-provenance-trust-store-v0`. It has one writer: `audio-provenance trust export-anchor` emits
  v1, and no v0 writer exists anywhere in the workspace outside test fixtures. `audio-provenance-wasm` has
  one reader, `FileTrustStore`, so it accepts v0 only. **OBSERVED**: handing the CLI's own
  `store.json` to `verify()` in Node raises `option_invalid: option trust_store is invalid: missing
  field 'signer_id'`. The Node test suite passes because its fixture is a hand-written v0 file, and
  the step-(i) verdicts above were produced with a v0 store built from the run's signer id.

- **`--null-test` gates soft bindings from reaching `verified`.** The full Watermark null artifact
  now covers 1,000 unmarked tracks across all 37 channels: **OBSERVED from the versioned artifact,**
  0/37,000 accepts and rule-of-three 95% upper bound 0.003003. Any demo still carrying the older
  370-trial pilot report must be repointed explicitly; this baseline does not silently rewrite a
  packaged demo asset.

- **The reference-constellation false-positive measurement was not re-run here, and it does not
  cover the case above.** REPORTED: 812 ordered pairs, 0 affirmations, 95% upper bound 0.0037, from
  `crates/apw-trace/tests/fingerprint_null_test.rs`, which is `#[ignore]`d as expensive and was left
  ignored. Those pairs are two *different recordings*. The interior-substitution case is a signed
  work with material swapped into it, which that harness never presented, and the span-based
  coverage it priced is the same term that is structurally blind to it. So 0.0037 is not a bound on
  the failure above. That is the concrete reason the repair needs a fresh measurement rather than a
  threshold move.

- **Acoustic re-recording is unsupported.** `capabilities()` returns the literal `"unsupported"`.
  The bench's acoustic rows are a **simulated** path and are labelled as such in the report; they
  are not a speaker-to-microphone measurement. REPORTED, and the report's own footnote says so.

- **Perceptual figures are segmental SNR and a noise-to-mask ratio.** Neither is PEAQ and neither is
  a listening test. Unchanged.

- **`identity` is `Option`.** A self-generated key proves key possession, not identity;
  verification reports `not_established` until a trust store anchors the signer.

- **Route 3(c) is unreachable below `MIN_QUERY_SECONDS`, 5 seconds**, and records signed before the
  `fingerprint` block existed carry no constellation, so a transcode of one still reports `changed`.
  Re-sign to get the soft binding. Forward-only, no migration.

- **The reference constellation is linear in duration**, about 62 bytes per second, capped at
  262144 peaks (roughly two hours). Fine for masters and stems.

- **`apw_watermark_candidate_cap_reached` fires on a 4-minute input.** **OBSERVED** in the Node result
  for the 240 s master: "peaks above the sync threshold were dropped unexamined; the recovery figure
  for this input is a cap artifact". The candidate cap is tuned for shorter items; on a full-length
  track the reported recovery figure is a floor, not the detector's real reach.
