# Multi-Layer Edit Observation Architecture

> **Status: partially shipped.** Of the layers below, the plugin audio path
> (Layer 1), the project file differ (Layer 3, `.als` parsing feeding
> `session_facts` and `project_diff` evidence into every manifest), and
> filesystem observation (Layer 5, sample import watching) are implemented.
> `daemon/input_capture/` and `daemon/screen_observer/` are unimplemented
> stubs, so correlation rules that depend on them cannot fire.
> The demo makes no claims that rest on this document.

## Problem Statement

A VST3 plugin can only observe audio buffers flowing through it. It cannot see
what the user did in the DAW: cutting a clip, adjusting a fader, adding an
effect, moving a region, or importing a sample onto a specific track.

Most commercial DAWs (Ableton Live in particular) expose no public API for
observing edit operations. The internal state of the application is a black box.

This document defines a multi-layer observation architecture that achieves
granular edit detection *without DAW cooperation* by instrumenting the layers
beneath and around the application: the operating system input pipeline, the
project filesystem, and the screen output.

## Design Principles

1. **Never claim what was not observed.** Every datum carries a proof level.
2. **No single layer proves an edit.** Confidence comes from temporal
   correlation across independent layers.
3. **Efficiency over fidelity.** Extract structured data early, discard raw
   captures immediately. Never store a screenshot when a feature vector
   suffices.
4. **Operate without DAW permission.** Every layer uses public OS APIs or
   filesystem observation. No reverse engineering, no memory inspection, no
   private API abuse.
5. **Degrade gracefully.** If a layer is unavailable (user denies accessibility
   permission, project format is unknown), the remaining layers continue and
   the evidence record notes the gap.

## Architecture Overview

```
┌──────────────────────────────────────────────────────────────┐
│                    Correlation Engine                         │
│  Fuses temporally-aligned events from all layers into        │
│  composite edit evidence with confidence scoring             │
├──────────┬──────────┬──────────┬──────────┬─────────────────┤
│  Layer 1 │  Layer 2 │  Layer 3 │  Layer 4 │  Layer 5        │
│  Audio   │  Input   │  Project │  Screen  │  Sample         │
│  Buffer  │  Capture │  Differ  │  Observer│  Watcher        │
│  (VST3)  │(CGEvent) │(.als XML)│(CoreGfx) │  (filesystem)   │
└──────────┴──────────┴──────────┴──────────┴─────────────────┘
     │           │          │          │            │
     │     keystroke/    project    feature       file hash
     │     mouse/focus   structure  vectors      + fingerprint
     │     events        diffs      (not pixels)
     │
   hash chain + spectral features + transport + MIDI
```

All layers feed events into a shared JSONL evidence stream via the daemon. The
correlation engine operates on this unified stream.

---

## Layer 1: Audio Buffer Observation (Implemented)

**Source:** VST3 capture plugin

**Mechanism:** Lock-free ring buffer consumed by a background thread that
computes per-window (4096 sample) features.

**Events produced:**

| Event | What it proves |
|-------|----------------|
| `buffer_hash` | Specific audio content existed at a specific moment in a specific order (chained SHA-256) |
| `audio_transition` | Silence began or ended at a precise sample position |
| `spectral_shift` | The frequency content changed significantly between adjacent windows |
| `transport_change` | The DAW started/stopped playback, changed BPM, entered record mode |
| `midi_event` | A MIDI note or controller message passed through the plugin |
| `session_config_change` | Sample rate, channel count, or buffer size changed |

**Proof level:** `directly_observed`

**Correlation value:** Provides the ground truth of *what audio existed when*.
Other layers explain *why* it changed.

---

## Layer 2: OS-Level Input Capture

**Source:** macOS `CGEventTap` (Quartz Event Services)

**Mechanism:** A user-space event tap registered for passive observation of
keyboard and mouse events systemwide, filtered to events where the target DAW
window is focused.

**Why this works:** This is the same technique used by the CPoE engine
(`crates/cpoe/src/platform/macos/`) for authorship attestation in text editors.
It observes the *input* to the application without needing the application's
cooperation.

### What is captured

| Signal | Data extracted | Not stored |
|--------|---------------|------------|
| Key down/up | Timestamp (ns), virtual keycode, modifier flags, inter-key interval | Raw character (privacy) |
| Mouse click | Timestamp, screen coordinates, click count, button | Continuous position stream |
| Mouse drag | Start/end coordinates, duration, modifier flags | Per-pixel path |
| Scroll | Timestamp, delta X/Y, phase (begin/momentum/end) | Raw event stream |
| Focus change | Timestamp, bundle ID, window title, PID | Window contents |

### Privacy constraints

- **No character capture.** Only virtual keycodes are recorded. The system
  knows "the user pressed key 0x00 (A) with no modifiers" but does not record
  the resulting character, which depends on input method and keyboard layout.
- **Coordinates are zone-bucketed.** Screen coordinates are mapped to a grid
  (e.g., 8x6 zones relative to the DAW window bounds) before storage. The raw
  pixel coordinates are discarded. This preserves spatial intent ("user clicked
  in the arrangement area") without enabling pixel-level reconstruction.
- **No continuous mouse tracking.** Only discrete events (click, drag
  start/end, scroll) are recorded. Idle mouse position is not captured.

### Keyboard shortcut recognition

The most valuable signal from input capture is **keyboard shortcuts**. DAW edit
operations are overwhelmingly triggered by shortcuts:

| Shortcut (Ableton) | Probable edit operation |
|---------------------|----------------------|
| Cmd+C / Cmd+V | Copy / Paste clip or selection |
| Cmd+X | Cut |
| Cmd+Z / Cmd+Shift+Z | Undo / Redo |
| Cmd+D | Duplicate |
| Cmd+E | Split clip at cursor |
| Cmd+J | Consolidate clips |
| Cmd+Shift+M | Insert MIDI clip |
| Delete / Backspace | Delete selection |
| Tab | Toggle session/arrangement view |
| Cmd+L | Toggle loop |
| 0-9 | Various (depends on context) |

A **shortcut classifier** maps `(modifier_flags, keycode)` tuples to probable
edit operations. The classifier is DAW-specific and configurable (different
mapping for Logic, Reaper, etc.). The output is a structured event:

```json
{
  "event_type": "input_shortcut",
  "proof_level": "inferred",
  "timestamp_ns": 1716700800000000000,
  "daw_bundle_id": "com.ableton.live",
  "shortcut": "cmd+e",
  "probable_operation": "split_clip",
  "confidence": 0.85,
  "zone": [4, 2],
  "notes": ["Shortcut-to-operation mapping is probabilistic. The user may have remapped keys."]
}
```

**Proof level:** `inferred` -- the system observed the input, not the result.

### Behavioral fingerprinting (future)

Following the CPoE pattern, accumulated keystroke timing data can produce a
behavioral fingerprint:

- **Inter-key interval (IKI) distribution:** Mean, std, skewness, kurtosis
- **Pause signature:** Sentence-pause vs. thinking-pause durations
- **Zone transition profile:** Which keyboard regions the user moves between
- **Dwell time:** How long keys are held

This fingerprint can distinguish between a human editing session and
programmatic/scripted automation, with proof level `inferred`.

### Platform requirements

- macOS: Requires Accessibility permission (`kAXTrustedCheckOptionPrompt`).
  The daemon should check on startup and degrade gracefully if denied.
- Linux (future): `evdev` or `X11` event capture.
- Windows (future): `SetWindowsHookEx` with `WH_KEYBOARD_LL` / `WH_MOUSE_LL`.

### Events produced

| Event | Proof level |
|-------|-------------|
| `input_shortcut` | inferred |
| `input_keystroke_batch` | directly_observed |
| `input_mouse_click` | directly_observed |
| `input_mouse_drag` | directly_observed |
| `input_focus_change` | directly_observed |
| `input_behavioral_fingerprint` | inferred |

---

## Layer 3: Project File Differ (Implemented)

**Source:** DAW project file on disk

**Mechanism:** Filesystem watcher monitors the project file (`.als` for
Ableton, `.logicx` for Logic, `.rpp` for Reaper). On each save, the daemon
decompresses and parses the file, computes a structural diff against the
previous snapshot, and emits change events.

### Ableton Live `.als` format

An `.als` file is a gzip-compressed XML document. Key structural elements:

```
<Ableton>
  <LiveSet>
    <Tracks>
      <AudioTrack Id="...">
        <DeviceChain>
          <MainSequencer>
            <ClipSlotList>           ← Session view clips
            <ArrangerAutomation>     ← Arrangement automation
            <Sample>
              <FileRef>              ← Sample file references
          <AudioToAudioDeviceChain>
            <Devices>                ← Effect chain (EQ, Compressor, etc.)
        <Name Value="..."/>
        <TrackVolume Value="..."/>
        <TrackPanning Value="..."/>
      <MidiTrack>
        <MidiClip>
          <Notes>                    ← Individual MIDI notes
          <WarpMarkers>              ← Time-stretch markers
    <Transport>
      <LoopOn Value="..."/>
      <LoopStart Value="..."/>
      <LoopLength Value="..."/>
      <CurrentTime Value="..."/>
    <MasterTrack>
    <Locators>                       ← Arrangement markers
```

### Diff strategy

The differ does **not** store or compare raw XML. It extracts a **structural
fingerprint** per save:

```python
@dataclass(frozen=True)
class ProjectSnapshot:
    track_count: int
    track_names: tuple[str, ...]
    clip_count: int                    # total across all tracks
    clip_hashes: frozenset[str]        # hash of (track_id, position, length, sample_ref)
    device_chain_hashes: frozenset[str] # hash of device chain per track
    automation_point_count: int
    midi_note_count: int
    sample_refs: frozenset[str]        # referenced sample file paths
    transport_bpm: float
    transport_loop_on: bool
    transport_loop_range: tuple[float, float]
    locator_count: int
    file_hash: str                     # SHA-256 of the entire .als file
```

A diff between two snapshots produces:

```python
@dataclass
class ProjectDiff:
    tracks_added: list[str]
    tracks_removed: list[str]
    clips_added: int                   # new clip hashes not in previous
    clips_removed: int
    clips_modified: int                # same position, different hash
    devices_changed: list[str]         # track names where device chain hash changed
    automation_points_delta: int
    midi_notes_delta: int
    samples_added: frozenset[str]      # new sample references
    samples_removed: frozenset[str]
    bpm_changed: bool
    loop_changed: bool
    locators_delta: int
```

### What this reveals

| Change detected | What it means |
|----------------|---------------|
| `clips_added > 0` | User recorded, pasted, or imported a new clip |
| `clips_removed > 0` | User deleted a clip |
| `clips_modified > 0` | User moved, resized, or edited a clip's content |
| `devices_changed` | User added/removed/reordered effects on a track |
| `samples_added` | User imported a new sample into the project |
| `midi_notes_delta != 0` | User edited MIDI content |
| `automation_points_delta` | User drew or edited automation |
| `bpm_changed` | User changed the project tempo |

### Limitations

- **Save-granularity only.** Changes between saves are invisible. If a user
  makes 50 edits then saves, the diff shows the aggregate, not the sequence.
- **Format may change.** Ableton does not document or guarantee the `.als`
  schema. The parser must handle unknown elements gracefully.
- **Cmd+S frequency varies.** Some users save after every edit; others save
  once at the end. The diff is only as granular as the user's save habits.

**Mitigation:** Combine with Layer 2 (input capture). Between saves, input
capture provides the edit sequence. At save time, the diff confirms the
aggregate result. Together they reconstruct the timeline.

### Events produced

| Event | Proof level |
|-------|-------------|
| `project_save_detected` | directly_observed |
| `project_diff` | directly_observed (structure changed) / inferred (what the change means) |
| `project_sample_ref_added` | directly_observed |
| `project_sample_ref_removed` | directly_observed |

---

## Layer 4: Screen Observation

**Source:** macOS screen capture APIs (`CGWindowListCreateImage`,
`SCScreenshotManager`, or `ScreenCaptureKit`)

**Mechanism:** Periodic capture of the DAW window, converted to structured
feature vectors, with the pixel data discarded immediately.

### Efficiency-first design

Screen capture is the most resource-intensive layer. The design must ensure:

1. **No screenshots are stored.** Pixel data exists only in a transient buffer
   during feature extraction (< 50ms lifetime).
2. **Capture rate is low.** Default: 1 capture per 2 seconds. During detected
   activity (recent keystrokes or audio transitions): 1 per 500ms.
3. **Resolution is reduced before analysis.** Capture at 50% resolution or
   quarter-resolution of the DAW window. We need layout, not pixels.
4. **Region-of-interest (ROI) cropping.** Only capture and analyze the regions
   that contain edit-relevant information, not the entire window.

### Processing pipeline

```
Capture (low-res screenshot of DAW window)
    │
    ├─► Downscale to working resolution (e.g., 480x270)
    │
    ├─► Region extraction
    │   ├─ Arrangement/timeline region (top ~60% of window)
    │   ├─ Mixer/device region (bottom ~30%)
    │   └─ Transport bar (top or bottom strip)
    │
    ├─► Per-region feature extraction
    │   ├─ Color histogram (16-bin per channel, quantized)
    │   ├─ Edge density (Sobel gradient magnitude, single scalar)
    │   ├─ Horizontal structure hash (row-averaged luminance, 64-byte vector)
    │   ├─ Vertical structure hash (column-averaged luminance, 64-byte vector)
    │   └─ Perceptual hash (pHash: 64-bit DCT-based hash of the region)
    │
    ├─► Diff against previous frame's features
    │   ├─ Hamming distance of perceptual hashes (0-64 bits)
    │   ├─ Histogram intersection (0.0-1.0 similarity)
    │   ├─ Edge density delta
    │   └─ Structure hash cosine similarity
    │
    ├─► Change classification
    │   ├─ "arrangement_changed" if timeline region pHash distance > threshold
    │   ├─ "mixer_changed" if mixer region pHash distance > threshold
    │   ├─ "transport_changed" if transport bar features shifted
    │   ├─ "view_switched" if all regions changed simultaneously (Tab key)
    │   └─ "no_change" if all distances below threshold
    │
    └─► Discard pixel buffer (CRITICAL: pixels never reach disk or network)
```

### What is stored per frame

```json
{
  "event_type": "screen_observation",
  "proof_level": "inferred",
  "timestamp_ms": 1716700800000,
  "daw_window_bounds": [0, 0, 1920, 1080],
  "regions": {
    "arrangement": {
      "phash": "a4c3b2f1e8d70956",
      "edge_density": 0.342,
      "histogram_similarity_vs_prev": 0.87,
      "phash_distance_vs_prev": 12
    },
    "mixer": {
      "phash": "f0e1d2c3b4a59687",
      "edge_density": 0.156,
      "histogram_similarity_vs_prev": 0.99,
      "phash_distance_vs_prev": 2
    },
    "transport": {
      "phash": "0123456789abcdef",
      "phash_distance_vs_prev": 0
    }
  },
  "classification": "arrangement_changed",
  "notes": ["Derived from feature vectors. No pixel data stored."]
}
```

**Total storage per observation: ~500 bytes.** Compare to a raw screenshot at
1920x1080 RGBA: ~8 MB, or compressed PNG: ~500 KB. The feature extraction
reduces data volume by **1000x** while preserving the signal we need.

### Region calibration

The screen observer must know where the DAW's UI regions are. Two approaches:

1. **Template matching at startup.** On first capture, locate the arrangement
   view, mixer, and transport bar using known visual landmarks (Ableton's dark
   theme has distinctive color bands). Cache the region bounds.
2. **User-assisted calibration.** Ask the user to identify regions once. Store
   the bounds relative to window size so they scale with window resizing.

If calibration fails, the observer falls back to whole-window feature
extraction (less granular but still useful for change detection).

### Adaptive capture rate

```
Base rate: 1 capture / 2000ms

If (input_capture reported keystroke within last 1000ms):
    rate = 1 capture / 500ms       # user is actively editing

If (audio_transition event within last 500ms):
    rate = 1 capture / 500ms       # audio changed, check what happened

If (no input or audio events for 10s):
    rate = 1 capture / 5000ms      # user is idle, save resources

If (DAW window is not focused):
    rate = 0                        # don't capture other apps
```

### Resource budget

| Metric | Target |
|--------|--------|
| CPU per capture cycle | < 5ms (downscale + feature extraction) |
| Memory (transient pixel buffer) | < 4 MB (quarter-res RGBA) |
| Memory (feature storage) | < 1 KB per observation |
| Disk I/O | 0 bytes for pixels; ~500 bytes/observation for features |
| Captures per minute (active editing) | 120 (every 500ms) |
| Captures per minute (idle) | 12 (every 5s) |

### Events produced

| Event | Proof level |
|-------|-------------|
| `screen_observation` | inferred |
| `screen_arrangement_changed` | inferred |
| `screen_mixer_changed` | inferred |
| `screen_view_switched` | inferred |

---

## Layer 5: Sample Folder Watcher (Implemented)

**Source:** Configured sample import directories

**Events produced:**

| Event | Proof level |
|-------|-------------|
| `sample_file_observed` | directly_observed |
| `ingredient_correlation` | inferred |

See existing `daemon/sample_watcher/` and `daemon/evidence_receiver/correlation.py`.

---

## Correlation Engine

The correlation engine is the intelligence layer. It consumes the unified event
stream from all layers and produces **composite edit evidence** -- events that
no single layer could produce alone.

### Temporal alignment

All events carry timestamps. The engine maintains a sliding window (default: 2
seconds) and groups events that fall within the same window. A group of
temporally-aligned events from multiple layers constitutes a **correlation
candidate**.

### Correlation rules

Each rule matches a pattern across layers and emits a composite event with a
confidence score.

#### Rule: Clip paste detected

```
IF   input_shortcut(cmd+v) at time T
AND  audio_transition(silence_to_audio) at time T ± 500ms
AND  spectral_shift at time T ± 500ms
THEN emit composite_edit(type="clip_paste", confidence=0.85)
```

#### Rule: Clip delete detected

```
IF   input_shortcut(delete) at time T
AND  audio_transition(audio_to_silence) at time T ± 500ms
THEN emit composite_edit(type="clip_delete", confidence=0.80)
```

#### Rule: Effect added/changed

```
IF   screen_mixer_changed at time T
AND  spectral_shift at time T ± 2s
AND  NO audio_transition at time T ± 2s  (audio continued, just changed character)
THEN emit composite_edit(type="effect_change", confidence=0.70)
```

#### Rule: Sample imported

```
IF   sample_file_observed at time T
AND  project_sample_ref_added matching same filename at time T2 (next save)
AND  ingredient_correlation at time T3 (stream matches sample fingerprint)
THEN emit composite_edit(type="sample_import_confirmed",
                         confidence=0.90,
                         ingredient_sha256=<sample hash>)
```

#### Rule: Arrangement edit (structural)

```
IF   project_diff(clips_added > 0 OR clips_modified > 0) at save time T
AND  screen_arrangement_changed within 10s before T
AND  input_keystroke_batch with edit shortcuts within 30s before T
THEN emit composite_edit(type="arrangement_edit",
                         confidence=0.75,
                         clips_added=N, clips_modified=M)
```

#### Rule: Undo detected

```
IF   input_shortcut(cmd+z) at time T
AND  audio content hash at T+500ms matches a hash from earlier in the chain
THEN emit composite_edit(type="undo",
                         confidence=0.90,
                         reverted_to_window=<matching window index>)
```

### Confidence scoring

Each correlation rule has a base confidence. Modifiers adjust it:

| Factor | Adjustment |
|--------|-----------|
| More layers agree | +0.05 per additional confirming layer |
| Tighter temporal alignment (< 200ms) | +0.05 |
| Loose temporal alignment (> 1s) | -0.10 |
| Missing layer (permission denied) | -0.05 (noted in evidence) |
| Contradictory signal | -0.15 (e.g., shortcut says paste but audio went silent) |

Confidence is clamped to [0.0, 1.0]. Events below 0.5 confidence are recorded
but flagged as low-confidence.

### Composite edit event structure

```json
{
  "event_type": "composite_edit",
  "proof_level": "inferred",
  "timestamp_ms": 1716700800000,
  "edit_type": "clip_paste",
  "confidence": 0.90,
  "contributing_events": [
    {"layer": "input_capture", "event_type": "input_shortcut", "timestamp_ms": 1716700799950},
    {"layer": "audio_buffer", "event_type": "audio_transition", "timestamp_ms": 1716700800020},
    {"layer": "audio_buffer", "event_type": "spectral_shift", "timestamp_ms": 1716700800020}
  ],
  "notes": [
    "Three layers corroborated within 70ms window.",
    "No project save occurred in this window; structural confirmation pending."
  ]
}
```

### State machine

The correlation engine maintains a lightweight state machine per track/stem:

```
               ┌──────────────────────────────┐
               │         IDLE                  │
               │  (silence, no recent edits)   │
               └──────┬───────────────────────┘
                      │ audio_transition
                      │ (silence_to_audio)
               ┌──────▼───────────────────────┐
               │       ACTIVE                  │
               │  (audio playing, observing)   │◄──── spectral_shift
               └──────┬───────────────────────┘      (stays ACTIVE)
                      │ audio_transition
                      │ (audio_to_silence)
               ┌──────▼───────────────────────┐
               │       EDITING                 │
               │  (recent edit detected,       │
               │   awaiting confirmation)      │
               └──────┬───────────────────────┘
                      │ project_save OR timeout(30s)
                      │
               ┌──────▼───────────────────────┐
               │       CONFIRMED / IDLE        │
               │  (edit committed to evidence) │
               └──────────────────────────────┘
```

---

## Evidence Chain Integrity

### Hash chain continuity

The audio buffer hash chain (Layer 1) serves as the temporal backbone. Every
composite edit event references the window hash at the time of detection. This
anchors inferred events to the tamper-evident audio chain.

### Cross-layer binding

Each composite event includes the contributing event IDs from each layer. An
auditor can verify:

1. The audio hash chain is unbroken (no windows removed or reordered).
2. Each composite event references real events in the hash chain.
3. The temporal alignment is consistent (contributing events are within the
   declared correlation window).
4. The confidence score is justified by the contributing evidence.

### Missing layer handling

If a layer is unavailable (e.g., user denied accessibility permission for input
capture), the evidence record includes:

```json
{
  "event_type": "layer_unavailable",
  "proof_level": "directly_observed",
  "layer": "input_capture",
  "reason": "accessibility_permission_denied",
  "timestamp_ms": 1716700800000,
  "impact": "Edit type inference will have reduced confidence. Shortcut recognition unavailable."
}
```

This preserves the honesty model: the system never silently degrades.

---

## DAW Compatibility Matrix

| DAW | Audio buffer | Input capture | Project differ | Screen observer | ARA (future) |
|-----|:---:|:---:|:---:|:---:|:---:|
| Ableton Live | VST3 | CGEventTap | .als (gzip XML) | Yes | No |
| Logic Pro | VST3/AU | CGEventTap | .logicx (package) | Yes | Yes |
| Reaper | VST3/CLAP | CGEventTap | .rpp (plaintext) | Yes | Yes |
| Studio One | VST3 | CGEventTap | .song (SQLite) | Yes | Yes |
| Cubase/Nuendo | VST3 | CGEventTap | .cpr (binary) | Yes | Yes |
| Bitwig | VST3/CLAP | CGEventTap | .bwproject (binary) | Yes | No |
| Pro Tools | AAX only | CGEventTap | .ptx (binary) | Yes | No |
| FL Studio | VST3 | CGEventTap | .flp (binary) | Yes | No |

**Input capture and screen observation are DAW-agnostic.** Project differ
requires a parser per DAW format. Audio buffer observation requires the DAW to
support VST3 (or AU/CLAP/AAX with corresponding plugin builds).

---

## Implementation Phases

### Phase 1: Scaffold (current)

- Define interfaces for all layers
- Stub implementations with event structures
- Integration points for the correlation engine
- This document as the specification

### Phase 2: Input capture

- Implement CGEventTap observer for macOS
- Shortcut classifier for Ableton Live
- Behavioral fingerprint accumulator
- Tests with simulated event streams

### Phase 3: Project differ

- Ableton `.als` parser (gzip XML extraction)
- Structural snapshot and diff computation
- Filesystem watcher integration
- Tests with sample `.als` files

### Phase 4: Screen observer

- macOS window capture (CGWindowListCreateImage)
- Downscale and region extraction
- Feature vector computation (pHash, histograms, edge density)
- Adaptive capture rate controller
- Tests with sample screenshots

### Phase 5: Correlation engine

- Temporal alignment and windowing
- Rule-based correlation with confidence scoring
- Composite event emission
- State machine per stem
- End-to-end integration tests

### Phase 6: Hardware attestation

- Secure Enclave provider (macOS) via Security.framework
- TPM 2.0 provider (Linux) via tpm2-tools
- Software fallback for development
- Hash chain root binding to hardware keys
- Self-entangled cosignatures per checkpoint

### Phase 7: External time anchoring

- RFC 3161 TSA client (DigiCert, Sectigo, DFN)
- Roughtime client (Cloudflare, Google, int08h)
- Dual-anchor verification with configurable tolerance
- Periodic checkpoint anchoring (every N windows or M seconds)

### Phase 8: ARA integration (DAWs that support it)

- ARA host adapter in the capture plugin
- Direct edit operation events (no inference needed)
- Proof level `directly_observed` for ARA-sourced edits
- Fallback to inference layers for non-ARA DAWs

### Phase 9: Anti-forgery analysis

- Audio stream regularity detection (RMS, spectral, transition timing)
- Input behavior analysis (IKI distribution, pauses, fatigue, skewness)
- Hash chain integrity verification (continuity, duplicates, monotonicity)
- Suspicion scoring model following CPoE severity weighting

### Phase 10: C2PA manifest generation

- Manifest builder from composite evidence
- C2PA assertion mapping (c2pa.hash.data, c2pa.ingredient, c2pa.actions)
- Custom apw: namespace for proof levels and unobserved data
- JSON manifest export with explicit unknown/unobserved fields

---

## Appendix A: Event Taxonomy (Complete)

### Layer 1: Audio Buffer (directly_observed)

| Event | Required fields |
|-------|----------------|
| `buffer_hash` | `window_hash`, `prev_hash`, `rms_level`, `zero_crossing_rate` |
| `audio_transition` | `direction`, `boundary_hash` |
| `spectral_shift` | `prev_spectral_centroid_hz`, `new_spectral_centroid_hz` |
| `transport_change` | `transport_state` |
| `midi_event` | `midi_event_type`, `midi_channel` |
| `session_config_change` | `sample_rate_hz`, `channel_count` |

### Layer 2: Input Capture (directly_observed / inferred)

| Event | Required fields |
|-------|----------------|
| `input_shortcut` | `shortcut`, `probable_operation`, `confidence` |
| `input_keystroke_batch` | `count`, `duration_ms`, `mean_iki_ms` |
| `input_mouse_click` | `zone`, `button`, `click_count` |
| `input_mouse_drag` | `start_zone`, `end_zone`, `duration_ms` |
| `input_focus_change` | `bundle_id`, `window_title` |
| `input_behavioral_fingerprint` | `iki_mean`, `iki_std`, `sample_count` |

### Layer 3: Project Differ (directly_observed / inferred)

| Event | Required fields |
|-------|----------------|
| `project_save_detected` | `file_hash`, `file_size_bytes` |
| `project_diff` | `clips_added`, `clips_removed`, `clips_modified` |
| `project_sample_ref_added` | `sample_path` |
| `project_sample_ref_removed` | `sample_path` |

### Layer 4: Screen Observer (inferred)

| Event | Required fields |
|-------|----------------|
| `screen_observation` | `regions`, `classification` |
| `screen_arrangement_changed` | `phash_distance`, `timestamp_ms` |
| `screen_mixer_changed` | `phash_distance`, `timestamp_ms` |
| `screen_view_switched` | `from_view`, `to_view` |

### Layer 5: Sample Watcher (directly_observed / inferred)

| Event | Required fields |
|-------|----------------|
| `sample_file_observed` | `sha256`, `file_name` |
| `ingredient_correlation` | `sample_sha256`, `confidence` |

### Correlation Engine (inferred)

| Event | Required fields |
|-------|----------------|
| `composite_edit` | `edit_type`, `confidence`, `contributing_events` |
| `layer_unavailable` | `layer`, `reason`, `impact` |

### Hardware Attestation

| Event | Required fields |
|-------|----------------|
| `hardware_binding` | `chain_root_hash`, `device_id`, `signature_hex` |
| `hardware_cosignature` | `entangled_hash`, `content_hash`, `monotonic_counter` |

### Time Anchoring

| Event | Required fields |
|-------|----------------|
| `time_anchor` | `source`, `timestamp_ms`, `nonce_hex`, `response_hex` |
| `dual_anchor` | `tsa_proof`, `roughtime_proof`, `within_tolerance` |

### Forgery Analysis (inferred)

| Event | Required fields |
|-------|----------------|
| `forgery_analysis` | `suspicion_score`, `flags`, `sample_count` |

### ARA Integration (directly_observed)

| Event | Required fields |
|-------|----------------|
| `ara_region_added` | `region_id`, `timeline_position`, `duration` |
| `ara_region_removed` | `region_id` |
| `ara_region_modified` | `region_id`, `property`, `old_value`, `new_value` |
| `ara_modification_created` | `modification_id`, `source_id`, `type` |
| `ara_modification_changed` | `modification_id`, `property`, `delta` |
| `ara_source_added` | `source_id`, `file_ref`, `sample_rate_hz` |

### Manifest Generation

| Output | Format |
|--------|--------|
| `manifest.json` | C2PA-compatible crJSON with apw: extensions |

---

## Appendix B: Comparison with CPoE Approach

| Aspect | CPoE (text) | Audio Provenance (this system) |
|--------|-------------|-------------------------------|
| Primary input | Keystrokes (CGEventTap) | Audio buffers (VST3) |
| Content hashing | Document checkpoint chain | Audio window hash chain |
| Behavioral signal | IKI distribution, dwell/flight time | Spectral centroid, RMS, ZCR |
| Temporal proof | VDF (Verifiable Delay Function) | Dual-anchor (RFC 3161 + Roughtime) |
| Hardware binding | TPM/Secure Enclave cosignature | Secure Enclave / TPM self-entangled cosignatures |
| Edit granularity | Per-keystroke | Per-window (~93ms at 44.1kHz) |
| Structural observation | N/A (text is linear) | Project file diff, screen features |
| Forgery detection | IKI anomaly analysis | Audio stream + input behavior + hash chain integrity |
| Cross-signal correlation | Jitter binding (entropy entanglement) | Temporal alignment + confidence scoring |
| Manifest output | Evidence packet (CBOR/COSE) | C2PA-compatible crJSON manifest |

The core architectural insight from CPoE that applies here: **no single signal
proves anything; the combination of independent signals, temporally correlated
and cryptographically bound, creates evidence that is progressively harder to
forge as more layers participate.**
