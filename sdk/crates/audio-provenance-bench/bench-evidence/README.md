# Captured bench output

Real output, not illustration. Produced by `bench-evidence/../src` at the commit these files sit in,
on ffmpeg 8.1.2 (`/opt/homebrew/bin/ffmpeg`), aarch64-apple-darwin, seed 7450482583528238693. The
matching machine-readable JSON is written beside them into `bench-out/`, which is gitignored because
the per-trial rows run to hundreds of kilobytes.

Reproduce:

```sh
cargo run --release -p audio-provenance-bench -- --codec fixture_lsb16 --duration 10 --out-dir bench-out \
  --real-wav .../demo-output/samples/demo-source.wav \
  --real-wav .../demo-output/sessions/capture-20260830T021208Z-11629/exports/session-b-bounce.wav \
  --real-wav .../demo-output/nulltest-20260829/null-main.wav
```

## What each file proves

**`fixture_lsb16.txt`** — the deliberately weak fixture against the product thresholds, 10 corpus
items (7 synthesised, 3 real WAVs from the POC's `demo-output/`).

| | |
| --- | --- |
| identity | 1.000 exact recovery, PASS |
| requantize_16bit | 1.000, PASS — the fixture rides the 16-bit grid, so this is bit-transparent |
| crop 0.5 / 2 / 7.3 s | 1.000, PASS — the detector searches one repetition period, so a leading crop only rotates the phase |
| mp3 192/128/96/64, aac 128/64, opus 128/64 | 0.000, FAIL |
| all three acoustic rooms | 0.000 |
| worst-case chain limiter → mp3 128 → acoustic → mp3 192 | 0.000 |
| false positives | 0 accepts in 370 unmarked trials, 95% upper bound 0.0081 |

mp3 320 at 0.400 and aac 256 at 0.200 are real, not a leak: the fixture repeats a 96-bit word about
4,700 times across a 10 s file, and on quiet or sparse content enough LSBs survive a transparent
320 kbps encode for the majority vote to close. It still misses its 0.99 threshold and still fails.

**`fixture_lsb16-uniform-gate.txt`** — the same fixture with `--min-recovery 0.9`, which removes the
`ExpectedFailure` exemptions. The three acoustic rows and the worst-case chain now read FAIL rather
than "recorded, no threshold". This run also carries a genuine error row: the corpus is 5 s, so
`crop_7s3` cannot run and reports `channel_input_too_short` instead of vanishing. The verdict lists
the two causes separately — 32 rows missed a threshold, 1 row could not run — because those are not
the same kind of evidence.

**`fixture_always_accept.txt`** — the harness self-test. A detector that embeds nothing and returns a
fixed payload scores 1.000 exact recovery on all 37 channels and 1.000 false-positive rate on 252 of
259 unmarked trials. Every row reads FAIL. A bench that cannot fail this has been executed, not
validated.

**`fixture_silent.txt`** — the opposite pole. A detector that never answers scores 0.000 everywhere
with a 0.000 false-positive rate, and still fails.

## Two bugs these runs caught in the bench itself

Both were in code producing headline numbers, and both were found by the crate's own tests rather
than by inspection:

- **FFT bit reversal** used `usize::BITS` where the reversal was `u32`, so the reordering shift was
  58 instead of 26 and every index collapsed to zero. `convolve` returned noise for any impulse
  response over 64 taps — every acoustic room preset. The acoustic rows read 0.000 either way, which
  is exactly why `acoustic_presets_reverberate_in_proportion_to_their_rt60` pins the decay itself.
- **Masking units.** Band energies were raw DFT power while the threshold in quiet was per-sample
  power, roughly 5e5 apart, so the absolute-threshold floor never bound and a -93 dBFS residual
  scored +27 dB noise-to-mask. After normalising, the same residual reads -20 dB.

A third was found by review: the perceptual measure mono-summed before comparing, which halves a
residual living in one channel — precisely what a channel-asymmetric embedder produces. It now
measures per channel and reports the worst.
