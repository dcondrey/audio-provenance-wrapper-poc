# Ableton plug-in real-time safety audit

Audit date: 2026-09-01. Scope: the JUCE VST3/AU processor in `src/`, built from the current
workspace. This is a code audit plus executable `juce::AudioProcessorPlayer` host-callback,
lifecycle, and deterministic soak tests. It does not claim to reproduce Ableton-specific UI,
freeze, or flatten behaviour.

## Audio-thread roots and guarantee

The audited host callbacks are both float and double forms of `processBlock` and
`processBlockBypassed`, plus the templates they call: `observeAudioBuffer`, `handleBypassedBuffer`,
`passThrough`, `AudioObserver::pushAudioBlock`, `pushMidiMessages`, `updateTransportState`,
`updateSessionConfig`, `setBypassActive`, `recordBypassedBlock`, and
`recordExternalAudioDrop`.

For the supported mono-to-mono and stereo-to-stereo layouts, the processor never writes input
samples. The only potential output clear in `passThrough` is unreachable for a supported layout,
because input and output channel sets must be equal. Executable tests compare every sample byte for
float32 and float64, including negative zero, denormals and NaNs, at block sizes 1, 7, 64, 511, 512,
4096 and 8192. The same tests instrument global `new`/`new[]` on the callback thread and require zero
allocations.

The static audit in `scripts/audit_plugin_realtime.py` extracts every callback-reachable body and
rejects allocation, mutex/spin-lock use, filesystem access, JSON, networking, logging, SDK calls,
and known allocating JUCE operations. It is intentionally syntactic and therefore supplements,
rather than replaces, the executable allocation probe.

## Thread and component boundaries

| Component | Thread | Boundedness and ownership |
| --- | --- | --- |
| Processor observation core | audio callback | sample reads, preallocated float conversion, atomics, and bounded FIFO writes only |
| `AudioObserver` FIFO | audio producer / one observer consumer | fixed 65,536-sample audio storage and 256 MIDI records; overflow never waits and increments loss counters |
| Observer feature/hash/event builder | background observer | owns hashing, FFT, JSON construction and the observation-chain state |
| `EventEmitter` | background observer plus ACK receiver | nonblocking UDP send; congestion or daemon absence increments send failure and cannot reach audio |
| ACK transition logic | pure | `AcknowledgementLogic` validates scope/counters and returns a new value; no I/O or shared mutation |
| JUCE processor/editor | host/message thread except callbacks above | state/UI/filesystem work remains outside the callback |

## Lifecycle and fault rules

- A sample-rate, channel-count or block-size change is staged atomically and adopted by the
  observer. Queued audio from the previous configuration is discarded as one counted affected
  segment before feature/hash continuity resets; samples from two configurations are never spliced
  into one evidence window.
- `releaseResources`, processor `reset`, bypass entry and bypass exit publish an observation
  boundary. Bypassed buffers/samples are counted and make complete coverage impossible; they are
  not silently represented as observed audio.
- Oversized double blocks never resize the conversion buffer in the callback. Audio remains
  untouched, the unobserved samples are counted, and MIDI/transport telemetry still proceeds.
- Every instance creates independent plug-in and capture-session UUIDs, event sequence, ACK state,
  hash chain and queues. ACKs for another instance/session return `scopeMismatch` without mutation.
- ACK counters are nonnegative integers no larger than 2^53, contiguous sequence cannot exceed
  accepted sequence, and daemon ids are 1–128 bytes. Malformed input changes no state. A daemon id
  change resets its sequence epoch and increments the restart counter.
- UDP sends use a socket explicitly placed in nonblocking mode. Daemon absence and congestion are
  evidence-delivery failures only. The send path does not take JUCE's read lock merely to probe
  writability; doing so previously made the receiver thread suppress otherwise valid sends.
- Host state is capped at 4 KiB and must be a flat, version-1 JUCE XML envelope with exact boolean
  encodings and a bounded decimal timestamp. Malformed, nested, legacy, inconsistent and oversized
  state restores safe defaults and increments the rejection counter.
- Teardown signals network workers, caps their joins at 250 ms, caps the observer join at 500 ms,
  and flushes at most four pending evidence windows. Remaining windows are counted as dropped. The
  executable aggregate shutdown gate is 1.5 seconds.

## Automated evidence

From a configured build directory:

```sh
cmake --build build-rt-audit --target AudioProvenanceCaptureTests AudioProvenanceCapture_VST3
ctest --test-dir build-rt-audit --output-on-failure
```

`plugin_realtime_lifecycle` covers layouts, float/double pass-through, callback allocation,
block/sample-rate changes, bypass, FIFO saturation, multi-instance isolation, pure malformed/restart
ACK transitions, state fuzz seeds, and bounded shutdown. `plugin_realtime_source_audit` covers the
forbidden-call list. `plugin_headless_daw_matrix` drives the processor through
`juce::AudioProcessorPlayer` and a deterministic fake audio device. It covers daemon-before-plugin,
plugin-before-daemon, daemon restart, project state chunk save/delete/reload, host bypass/re-enable,
rapid variable-size offline callbacks, plug-in deletion, exact shutdown flushing, FIFO
conservation, bit-identical pass-through, and zero callback allocation.

The short soak runs in ordinary CI. `.github/workflows/headless-daw-soak.yml` schedules the same
binary for 28,800 wall-clock seconds on a dedicated `self-hosted`, `macOS`, `apw-soak` runner. The
gate requires zero steady-state live-allocation growth, zero real-time allocation violations, zero
transparency violations, and the exact `min(ready_windows, 4)` shutdown-flush bound. This automated
gate permanently replaces the former eight-hour manual Ableton playback/offline soak requirement.

Steady state is established before the measured window opens rather than assumed. The soak renders
one-second segments and quiesces the observer thread after each until two consecutive segments
retain nothing, then measures across the soak proper and compares both directions. The absorb phase
exists because several allocations on the audio path are one-time lazy initialisation -- the
observer's hash chain is empty until its first window, and JUCE interns an event's JSON property
names the first time that variant is emitted -- and the quiesce exists because a returned audio
callback does not mean the observer thread has finished accounting for its window. `lazy_init_bytes`
in the run's JSON reports what the absorb phase retained; it is diagnostic, not gated.

## Ableton-specific integration checks

Track duplication, bypass/re-enable specifically through Ableton's UI, freeze, flatten, export
overwrite, and unsupported render formats still depend on Ableton integration and are not inferred
from the lightweight host. Run those against the signed release candidate and record host version,
macOS version, plug-in format, sample format, layout, sample rate, buffer size, counters and any host
dropout report. They are compatibility checks, not an additional soak requirement. A failure may
recover or end as explicit partial/unknown coverage; it may never be silently called complete.
