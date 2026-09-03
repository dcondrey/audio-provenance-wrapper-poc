# audio-provenance-bench

The bench comes before the watermark it measures.

A mark that round-trips clean audio and dies on every real distribution path is indistinguishable
from a working one until something measures it. This crate is that something, and it was written and
validated against known-bad detectors before GenoMark existed.

## Running it

```sh
cargo run --release -p audio-provenance-bench -- --codec fixture_lsb16 --duration 10 \
  --out-dir bench-out \
  --real-wav /path/to/a.wav
```

Exit code 0 means the run passed every threshold it was given; 1 means it did not. The run writes
`<codec>.json` and `<codec>.txt` into `--out-dir` and prints the table on stdout.

| flag | meaning |
| --- | --- |
| `--codec` | `fixture_lsb16`, `fixture_always_accept`, `fixture_silent` |
| `--ffmpeg` | path to ffmpeg, default `/opt/homebrew/bin/ffmpeg` |
| `--duration` | seconds of synthetic corpus audio per item |
| `--real-wav` | a real WAV to add to the corpus; repeatable |
| `--min-recovery` | replace the product thresholds with one uniform gate on every channel |
| `--seed` | base seed; every cell seed derives from `(item, channel, base seed)` |
| `--no-perceptual` | skip the original-versus-marked measurement |

## What it measures

A corpus crossed with 37 channels. Per cell it embeds a known payload, degrades, and detects.

- **exact-payload recovery rate** — the pass/fail metric
- **bit error rate** over trials that returned a payload, printed beside the rate at which the
  detector declined, because either number alone is misleading
- **false-positive rate** — the same detector over UNMARKED audio through the same channel. This arm
  is mandatory. A detector with no measured false-positive rate is not a detector, it is a hope.
- **detection wall time**, and the frame delta the channel introduced

Every channel appears in the report. One that could not run, because its codec was unavailable,
appears as an explicit error row and fails the run. `channels_expected` and `channels_reported` are
both emitted so truncation is checkable rather than trusted.

## The channel matrix

`identity` · mp3 320/192/128/96/64 · aac 256/128/64 · opus 128/64 · resample via 44.1 k and 22.05 k ·
requantize to 16 and 8 bit · gain -12/-6/+6 dB and normalize-to-peak · crop 0.5/2/7.3 s · clock drift
±0.1 % with no pitch correction · additive noise at 40/30/20 dB SNR · lowpass 16 k and 11 k ·
compression · brickwall limiting · three simulated room presets · three chains, including the
realistic worst case `limiter -> mp3 128 -> acoustic -> mp3 192`.

The 7.3 s crop is in the set because it lands off every common analysis-frame grid, which is what
breaks naive synchronisation. A test pins that property so a later tidy-up cannot round it away.

Codec channels use ffmpeg, and the encoder is part of the channel identity: ffmpeg ships both a
native `aac` and an `aac_at` AudioToolbox encoder and they are different channels, so the encoder
name is in the row parameters. libopus always runs at 48 kHz; that row's decode leg resamples back to
the corpus rate, and it says so in its parameters.

The bench never realigns before detection. It records the frame delta instead, because sync is the
detector's job and hiding a time shift would flatter it.

## The acoustic rows are SIMULATED

`AcousticRerecord` convolves with a synthesised impulse response — exponentially decaying noise, with
a faster-decaying high band, plus discrete early reflections — then applies a fixed speaker/microphone
curve, Gaussian room noise, and a small clock drift.

**That is a model of a room, not a room.** It is not a substitute for playing a file through a
loudspeaker and capturing it with a microphone, and no output of this crate may be described as an
over-the-air measurement. The disclaimer is carried in the channel parameters and in the report body
so it cannot be separated from the number by copy-and-paste.

## The perceptual numbers are not PEAQ

`perceptual::measure` reports segmental SNR and a masking-threshold-weighted noise-to-mask ratio over
a half-Bark partition with a Schroeder spreading function, a fixed tonality assumption, and an
absolute-threshold curve anchored on the convention that digital full scale is 96 dB SPL. Every one of
those is an assumption, not a measurement. Neither number is ITU-R BS.1387, neither yields an ODG, and
neither replaces a blinded listening test. `perceptual::PERCEPTUAL_LIMITS` ships the same paragraph in
every report.

## The fixtures, and why they exist

Three deliberately simple codecs, named `fixture_*` so no reader can mistake one for GenoMark:

- `fixture_lsb16` — payload plus a sync word and CRC-16 in the least significant bit of the 16-bit
  quantisation, repeated to fill the file. Bit-exact through a transparent path, gone the moment
  sample values move.
- `fixture_always_accept` — embeds nothing, returns a fixed payload for any input. Against it the
  false-positive arm must read 1.0 and every row must FAIL despite perfect recovery.
- `fixture_silent` — embeds nothing, never answers. Every recovery cell must read 0.0.

A harness that cannot fail a known-bad implementation has been executed, not validated. Captured
output for all three is in `bench-out/`.

## Portability

Every process launch and filesystem touch goes through `ports::CommandRunner` and `ports::FileStore`.
No DSP, measurement or reporting code calls `std::process`, `std::fs` or a clock; `generated_at` is a
parameter. The runner itself is native-only behind the `native` feature, since timing a detector is
what it exists to do.
