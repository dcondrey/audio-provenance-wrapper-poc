---
title: "Blind recovery of audio watermarks over an acoustic path"
subtitle: "What the published literature actually reports"
author: "David Condrey, WritersLogic"
date: "31 August 2026"
---

## Summary

No published audio watermarking method reports a **blind** recovery rate over a
real speaker-to-microphone path.

Every physical-path figure in the literature is an *oracle* measurement: the
decoder is run at many candidate time offsets, each result is scored against the
known ground-truth payload, and the best score is reported. A deployed detector
has no ground truth to score against, so it cannot reproduce those numbers.

The two methods that use genuinely physical capture both do this, and both say so
in their own source. The methods that report strong "replay" robustness are
running a software simulation of a room, not a room.

This is not an argument that acoustic recovery is impossible. It is an
observation that the number which would establish it does not exist yet, and a
note on what it would cost to produce.

## The two physical-capture methods

**DeAR** (AAAI 2023) plays through a Sennheiser SP10 and captures on an
ATR2100-USB microphone. It reports bit accuracy of 99.18%, 98.55%, 93.40% and
92.68% at 5, 20, 50 and 100 cm, over 200 test tracks. Its ablation shows
reverberation is the load-bearing component: removing the environment
reverberation term from the training distortion layer drops 99.18% to 73.63%.

The paper states its synchronisation method verbatim:

> we shift the target re-recorded audio within a pre-defined range (3/44.1 s,
> 8/44.1 s) and calculate the corresponding bit recovery accuracy (ACC), in
> which the highest one is regarded as the final ACC.

That is a maximum over a 113 ms shift search, scored against the true bits. Room
type, RT60 and ambient noise floor are not stated anywhere in the paper. No code
or weights were released.

**DeepAWR** (Pattern Recognition, 2025) captures physically as well: a Windows
machine plays the marked file while a mobile device records. Its shipped result
logs read 0.99 and 1.00 bit accuracy.

Its extraction code performs the same oracle search, and it is visible in the
repository. `extract.py` sweeps 6000 sample offsets, decodes at each, computes
accuracy against the known payload, keeps the maximum, and exits early on a
perfect hit. A CRC verification function is called and its result is discarded
rather than used as the acceptance gate. The capture conditions, speaker,
microphone, room and distance, are documented nowhere in the repository.

## The methods that report strong replay robustness are simulating

**AWARE** (2026) reports 0.024 bit error rate under what it calls a replay
attack. The paper defines it as convolution with impulse responses from the
Aachen database, plus 50 dB Gaussian noise and a 50 Hz to 8 kHz band-pass. There
is no loudspeaker, no microphone, no distance and no clock drift. It is a test
against the same model of the acoustic path that a training distortion layer
would use.

Its miss rate is also worth noting: a false negative rate of 0.11 at 16 bits per
second, rising to 0.58 at 20 bits per second. Even in simulation, at the higher
rate the majority of marked clips are missed outright.

Under that same simulated replay, AudioSeal and WavMark both score a bit error
rate of 0.500 with a false negative rate of 1.00, which is chance.

## What is genuinely encouraging

RAW-Bench (Sony, Interspeech 2025) retrained existing architectures with reverb
inside the training loop. SilentCipher's architecture went from 0.45 to 0.95
full-message accuracy under strict simulated reverb. A waveform-domain model
improved far less, from 0.22 to 0.41.

That is direct published evidence that reverb robustness is trainable into a
spectral-domain architecture. It is simulated reverb, and the retrain used a
proprietary corpus, so it is a reason to run the experiment rather than a result
to rely on.

## Commercial licence position

Verified by reading the LICENSE files and model-card metadata directly, not
inferred from repository badges.

| Method | Code | Weights | Usable commercially |
|---|---|---|---|
| AudioSeal (Meta) | MIT | MIT since 2024-04-02, stated explicitly in the README | Yes. The only fully clean pipeline: code, weights, training code and an MIT ONNX export |
| SilentCipher (Sony) | MIT | No grant. The HuggingFace card declares no licence and the repo's MIT grant is scoped to "the code in this repository" | No, not without a licence line from Sony |
| DeepAWR | MIT declared | In-tree, same declaration, but the code derives from the unlicensed DeAR release, so the chain of title is defective | Reference document only |
| DeAR | None released | None released | No |
| AWARE | MIT | None exist. The repository constructs randomly initialised networks and optimises per clip | No |

AudioSeal is the only viable starting point. It is 16 kHz speech-trained, has no
room impulse response anywhere in its augmentation set, and scores at chance
under simulated replay, so it is a harness to fork rather than a solution.

## What would settle it

The missing artifact is a single number: a blind, CRC-gated detection rate on a
physical speaker-to-microphone path, with a false-positive rate measured over
unmarked audio at a threshold fixed in advance.

Producing it does not require a training budget. It requires three rooms, three
speakers, three microphones, measured RT60 and direct-to-reverberant ratios per
room, and a campaign across distances. Roughly two weeks. That measurement should
precede any serious model investment, because it is the number that says continue
or stop, and because a measured negative result is itself a contribution the
field currently lacks.

A realistic target is presence detection, "this audio carries a mark," at about
one metre in a quiet room. A recoverable payload at conversational distance is a
materially harder problem, and anything at venue distance or through a pocket is
not supported by any evidence I found.

## Bearing on public claims

Until that measurement exists, "survives re-recording" is not supportable by
anyone in this field, including any implementation I have built. I would treat
the claim carefully in public material until a rig has run.

---

*Every figure above is checkable at its source. The DeAR quotation is from the
published paper; the DeepAWR search behaviour is in `extract.py` in the public
repository; the licence positions are from the LICENSE files and HuggingFace API
metadata as of 31 August 2026.*
