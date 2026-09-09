# Runbook — Watermark-N Stage 0 physical capture campaign

Two weeks, three rooms, three speakers, three microphones. This is the campaign spec section 10
orders before any training step, and its outputs are inputs to everything after it.

Buy the list in `BOM.md` first. Read the *Day 1* section before anything else: it can stop the
programme in an afternoon for the price of a room booking.

---

## 0. Why the grid is staged rather than crossed

The full cross product of spec 10.1 is

    3 rooms × 3 speakers × 3 microphones × 5 distances × 2 SPL levels = 270 cells

and 200 clips through all of it, at both arms and 30 s per clip, is **108,000 captures ≈ 960 hours**.
That is not a two-week campaign, and no kill criterion asks for it. K4 needs ≥ 3 rooms × ≥ 2 speakers
× ≥ 2 microphones × ≥ 200 clips **at 1.0 m**; K6 needs a distance ladder; K0 needs impulse responses,
which are cheap. So `configs/stage0.example.yaml` stages the work:

| Stage | Cells | Captures | Playback hours |
|---|---|---|---|
| `rir_sweep` | all 270, × 3 repeats | 810 | **3.4** |
| `room_noise` | 3 rooms × 3 mics | 9 | 0.2 |
| `k4_corpus` | 3 rooms × 2 speakers × 2 mics, 1.0 m, both arms, 200 clips | 4,800 | **42.7** |
| `k6_ladder` | 1 room × 1 speaker × 1 mic, 5 distances, both arms, 40 clips | 400 | **3.6** |
| | | **6,019** | **≈ 50 h** |

Multiply by ~1.6 for room changes, distance changes, meter readings and handling: **about 80 hours of
attended time, ten working days.** `capture-rig plan --campaign <file>` prints these numbers for
*your* config before you record anything. Run it first.

Of the 4,800 `k4_corpus` captures, 2,400 are on the two `ingest` microphones — the phones — which this
process cannot drive. Section 5 explains how those are recorded in continuous takes and segmented,
which is what keeps them affordable.

Storage: roughly 24 GB at 24-bit 48 kHz mono (810 sweeps ≈ 1.5 GB, 5,200 corpus captures ≈ 22.5 GB),
plus the impulse responses.

---

## 1. Day 1 — measure the rooms before committing to them

Nothing here needs the corpus, and it can end the programme cheaply.

```bash
cd tools/capture-rig
uv sync
uv run capture-rig devices
```

Copy the exact device names into your campaign file's `output_device` and `input_device` fields.
Ambiguous names are refused rather than guessed at; running a whole campaign into the wrong microphone
is not recoverable after the fact.

Set the room up: speaker on a stand at ear height, microphone on axis at the marked distance, tape
down at 0.15 / 0.5 / 1.0 / 2.0 / 3.0 m. Then, **for each room**, sweep at 1.0 m and read K0:

```bash
uv run capture-rig run  --campaign configs/stage0.yaml --stage rir_sweep
uv run capture-rig k0   --campaign configs/stage0.yaml
```

`k0` exits **2** if the criterion fires. Read the JSON, not just the exit code:

- `rooms.<id>.median_rt60_seconds` — the treated room must be at or below **0.6 s** at 1.0 m.
- `rooms.<id>.median_drr_db` — at least one room must be at or above **0 dB** at 1.0 m.
- `untrustworthy_measurements` — **check this before believing any of the above.** A non-zero count
  means some responses were noise-limited or had a curved decay, and the verdict is provisional.
  Lengthen `sweep.silence_seconds`, raise `sweep.amplitude_dbfs`, or use a longer sweep, and re-run.
- `measurement_quality_note` — says the same thing in a sentence.

**If K0 fires on a real, well-measured room, stop. The programme ends here, before training.** That is
the criterion working, and it costs a week rather than fifteen.

If the treated room misses 0.6 s narrowly, panels and soft furnishings are the fix; re-measure before
concluding anything.

## 2. Day 1 — calibrate SPL, and refuse to proceed without a meter

```bash
uv run capture-rig calibrate --campaign configs/stage0.yaml \
    --room treated_small --speaker powered_monitor --microphone usb_condenser \
    --meter-dba 74.0 --meter 'Class 2 SLM, model and serial'
```

Hold the meter at the microphone's capsule, A-weighted, fast. The tool plays a 1 kHz tone — 1 kHz
because that is where A-weighting is unity, so your meter reading and the rig's A-weighted dBFS refer
to the same quantity by construction — and verifies three things:

1. the capture does not clip (peak at or below −0.5 dBFS);
2. the capture peaks inside −18 to −6 dBFS (adjust interface gain until it does);
3. **the capture is actually the tone.** At least half the captured power must sit within 25 Hz of
   1 kHz. This catches the failure you will otherwise not notice: a muted output, a microphone
   pointed at nothing, or a capture of the wrong device, all of which give a plausible level.

It prints a `spl_calibration:` block. **Paste it under that room in the campaign file.** The tool does
not edit your config for you.

Without `--meter-dba`, every SPL and background-dBA field in that room's captures is `null` with a
reason. That is honest, and it is also a campaign that reproduces the exact defect spec section 10
exists to fix. Get the meter.

Re-calibrate whenever the interface gain, the speaker, or the microphone changes.

## 3. Background noise, per room

```bash
uv run capture-rig run --campaign configs/stage0.yaml --stage room_noise
```

Sixty seconds of silence per room per microphone. Leave the room; a person breathing is 10 dB above a
quiet room's floor. Every later capture record in that room cites this measurement in
`condition.background_dba`.

## 4. The corpus, live microphones

Prepare the clips first. Both directories must hold **at least** the clip count the stage asks for,
already at the campaign's sample rate — the rig refuses to resample inside the playback path, because
that would put a conversion in the measured chain that the record does not describe.

- `corpus/marked/` — the marked clips.
- `corpus/unmarked/` — **the same number of unmarked clips.** Spec 10.2(e): the false-positive arm is
  not an afterthought and cannot be synthesised later. A corpus stage without an unmarked arm is
  refused at config load.
- `payloads.json` — `{"schema": "audio-provenance-capture-rig-payloads/1", "payloads": {"<clip_id>": "<hex>"}}`.
  This file is read **only when scoring**, never by a detector.

Then:

```bash
uv run capture-rig plan --campaign configs/stage0.yaml
uv run capture-rig run  --campaign configs/stage0.yaml --stage k4_corpus
```

`run` is resumable and idempotent. Interrupt it, change rooms, come back, run it again: complete cells
are skipped, and a cell whose WAV no longer matches the frame count and byte size its sidecar declared
is re-queued. `plan` prints any such cell under `RE-QUEUED` with the reason.

Move to the next distance or room, update nothing — the grid is already in the config — and run again.

## 5. The corpus, phone microphones

An iPhone and an Android phone are not CoreAudio inputs on this machine, so they are `ingest`
microphones: the operator records, and the rig registers the file with the identical condition record.
Recording 200 clips as 200 separate files by hand is not sensible, so record **one continuous take per
cell** and segment it.

1. Start the phone recording. Play the 200 marked clips back to back through the speaker, with about a
   second of silence between them. Stop the phone. Repeat for the unmarked arm.
2. Transfer the take, and convert it to 48 kHz WAV if the phone wrote anything else. Record that you
   converted it.
3. Segment it. **This is bookkeeping, and it uses the source clips, which are ground truth. It never
   touches detection** — see the firewall section of the README.

```bash
uv run capture-rig align --capture take.wav \
    --clip clip000=corpus/marked/clip000.wav \
    --clip clip001=corpus/marked/clip001.wav \
    ... \
    --write-segments segments/ --out alignment.json
```

Every segment reports a `peak_to_sidelobe` ratio and an `accepted` flag. A segment that is not
accepted was not confidently located; re-record rather than ingesting a guess.

4. Register each segment against its task id (from `capture-rig plan --json`):

```bash
for id in $(jq -r '.written_segments | keys[]' alignment.json); do
  uv run capture-rig ingest --campaign configs/stage0.yaml \
      --task "k4_corpus/treated_small/powered_monitor__iphone__1m__75dba/marked/$id" \
      --file "segments/$id.wav" --alignment alignment.json \
      --note 'iPhone on stand, on axis'
done
```

Pass `--alignment`. It refuses a `--task` that names a different clip from the one the segmenter
located, and records the segment's `peak_to_sidelobe` in the capture record as provenance. Without it
a single mistyped task id silently scores one phone capture against another clip's payload for the
rest of the campaign. The confidence figure is a provenance field and never a detector input.

An ingested capture is a first-class trial: same condition record, same report row, `capture.mode` is
the only difference.

## 6. The distance ladder

```bash
uv run capture-rig run --campaign configs/stage0.yaml --stage k6_ladder
```

Move the microphone, re-read the meter, re-run `calibrate` if the level changed, run again. K6 asks
for the largest distance at which the K4 threshold is met, and pools per distance across cells; a
distance carrying fewer than 200 trials reads `unevaluated` rather than contributing a verdict.

## 7. Impulse responses into training

```bash
uv run capture-rig analyse-sweeps --campaign configs/stage0.yaml
```

This deconvolves every stored sweep — rebuilding the inverse filter from the stimulus each capture
recorded, not from the config's current `sweep:` block — and writes

    <output_dir>/impulse_responses/**.wav          the measured responses
    <output_dir>/impulse_responses/**.json         RT60, octave-band RT60, DRR, THD, bulk delay
    <output_dir>/impulse_responses/manifest.jsonl  licence-gated, loads directly in the training tree

Point the training config's `rir.corpus_dirs` at that directory. Spec 9.2 ranks these responses first
among all impulse-response sources, above the MIT survey and EchoThief, because they were measured
with the exact speakers and microphones the product will face.

The per-response `thd_percent` is the measured harmonic distortion of that speaker at that level. It
is what parameterises distortion-layer stage D4 with a real figure instead of a guessed range, and it
comes free from the same sweep.

## 8. The report

```bash
uv run capture-rig report --campaign configs/stage0.yaml \
    --detector null \
    --thresholds thresholds.json --payloads payloads.json --out results/
```

**Run `--detector null` first.** It must produce `exact_recovery_rate` 0.0 and `false_positive.rate`
0.0 over the whole campaign. Anything else means the report arithmetic is manufacturing detections and
no number from it can be trusted.

Then the real detectors. Spec 10.3 makes AudioSeal and WavMark the baselines, run blind over these
same captures, and their result is the first blind physical detection rate for any licence-clean
watermark:

```bash
uv run capture-rig report --campaign configs/stage0.yaml \
    --detector 'exec:/path/to/audioseal-detect' \
    --thresholds audioseal-thresholds.json --payloads payloads.json --out results/audioseal/
```

**`thresholds.json` must be frozen before the run.** Spec 12.2: both the presence threshold and the
CRC gate are calibrated on ≥ 5000 unmarked *simulated* trials and only confirmed here. A threshold
fitted on these trials is the same oracle defect as an offset sweep wearing different clothes, and the
report would be worthless.

Read, in this order:

1. `totals.false_positive_accepts` — **if this is not zero, nothing else in the report is a result.**
2. `totals.false_positive_upper_bound_95` — 3/N, with N stated. Quote them together or not at all.
3. `kill_criteria.K4.verdict` — `pass`, `kill`, or `unevaluated` with `coverage_gaps` listing what the
   campaign still owes.
4. `kill_criteria.K5b` and `K6`.
5. The per-channel rows, which are where the distance and room dependence actually lives.

`report` exits 2 when any criterion reads `kill`.

## 9. What a pass entitles anyone to say

Exactly the numbers measured, with the room count, speaker and microphone count, distance, duration,
blind detection rate, false-positive rate and trial count attached (spec 12.4). Not "survives
re-recording". Not "works in a room". Until the Stage 2 gate passes, `capabilities()
.acousticRerecording` stays the literal `"unsupported"`.

Every published number this can be compared against is an oracle best-of-search maximum computed with
knowledge of the true bits. These are not. That difference is the entire point of the campaign, and it
means the number will be **lower** than anything in the literature — which is the correct outcome, not
a disappointing one.

---

## Schedule

| Day | Work |
|---|---|
| 1 | Devices, room 1 setup, sweeps, **K0**, calibration, background noise |
| 2 | Rooms 2 and 3: setup, sweeps, K0, calibration, background noise. `analyse-sweeps`; IR corpus done |
| 3 | Corpus preparation; `--detector null` dry run over a handful of captures |
| 4–7 | `k4_corpus`, live microphone, three rooms × two speakers |
| 8–9 | `k4_corpus`, phone microphones: continuous takes, `align`, `ingest` |
| 10 | `k6_ladder` |
| 11 | Re-run `plan --verify-digests`; re-capture anything re-queued |
| 12–13 | `report` with the null control, then AudioSeal and WavMark blind |
| 14 | Write up: K0, K4, K5b, K6, and the IR corpus handed to training |

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| `k0` reports `untrustworthy_measurements > 0` | Sweep too short, silent tail shorter than RT60, or playback too quiet | Raise `sweep.duration_seconds` to 10 s, `silence_seconds` above the room's RT60, `amplitude_dbfs` toward −6 |
| RT60 reads roughly double the expected value | Sweep too short or too narrow for the analysis band; `poor_fit` will be set | 10 s, 20 Hz–20 kHz |
| `calibrate` says the microphone is not hearing the tone | Muted output, wrong device, or microphone pointed away | Check `capture-rig devices`; re-aim |
| Every `spl_dba` is `null` | No `spl_calibration` on that room | Run `calibrate --meter-dba` and paste the block in |
| `run` fails with "declares no input_device" | The microphone is `mode: ingest` | Record on the device; use `capture-rig ingest` |
| `plan` shows cells as `corrupt` | Interrupted write, or a file changed under the store | They are already re-queued; just run again |
| `align` rejects a segment | The clip is not confidently in the take | Re-record the take; do not ingest a guess |
| `report` shows `unevaluated` on every criterion | The campaign has not yet captured the coverage a verdict needs | Read `coverage_gaps`; it lists exactly what is owed |
