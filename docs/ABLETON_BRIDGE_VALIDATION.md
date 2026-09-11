# Ableton Semantic Bridge Validation

This validates the Phase 2 Max for Live / Live API research spike. It does not modify the VST plugin, add C2PA signing, host plugins, or claim full Ableton provenance.

## Setup

1. Open Ableton Live with Max for Live available.
2. Create or open a Live Set with at least two tracks.
3. Add one or more Session View clips.
4. Add at least one Ableton-native device and, if available, one third-party plugin device.
5. Create a Max for Live device on any track.
6. Add a Max `js` object and load `ableton_bridge/live_api_probe.js`.
7. Trigger the script after the device is initialized. The safest Max patch pattern is:

```text
[live.thisdevice]
|
[deferlow]
|
[js live_api_probe.js]
```

## Run A One-Time Probe

Send `bang` to the `js live_api_probe.js` object.

Expected result:

- JSON is printed in the Max console.
- The JS object's outlet emits the same JSON string.
- The JSON contains `event_type: "ableton_session_probe"`.
- The JSON contains `proof_level: "directly_observed_via_live_api"`.
- Tracks, track names, devices, parameters, selected track, and accessible clips are reported where Live API exposes them.

## Test Real-Time Observation

Send `start_observing` to the JS object.

Then:

1. Select a different track in Ableton.
2. Add or remove a track.
3. Rename a track.
4. Add or remove a Session View clip.
5. Add or remove a device.

Expected result:

- The Max console logs observed changes for registered observers.
- The probe reruns and emits fresh JSON.

This only validates that some Live API changes can be observed. It does not prove complete DAW event capture.

## Audio Clip Source Path Test

1. Drag an audio file into a Session View clip slot.
2. Run the probe.
3. Find the clip entry in the JSON.
4. Check:

```json
{
  "is_audio_clip": true,
  "source_file_path": "...",
  "source_file_accessible": true
}
```

If `source_file_path` is `null` or `source_file_accessible` is `false`, record that as an observed limitation for that Live version, file type, or clip state.

## Proof-Level Review

Confirm the output does not claim:

- full Ableton session provenance
- exact source-to-export contribution
- sample ownership or license status
- third-party plugin internal preset state
- wrapper or mini-host capture
- C2PA signing

Fields returned by Live API may be labeled `directly_observed_via_live_api`. Any relationship to plugin activity, sample watcher evidence, or final exported audio must be labeled `inferred` unless a later component directly observes it.

## Stop Observation

Send `stop_observing` to the JS object before deleting the Max device or closing the Set.
