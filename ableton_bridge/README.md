# Ableton Semantic Bridge Research Spike

This folder contains a Phase 2 research prototype for probing Ableton Live session metadata through Max for Live JavaScript and the Live API.

This is not production provenance capture. It does not modify the VST plugin, sign C2PA data, host plugins, or claim full DAW capture.

## Files

- `live_api_probe.js` is a Max for Live JavaScript proof-of-concept.
- `example_session_probe_output.json` shows the intended JSON shape.
- `docs/ABLETON_BRIDGE_VALIDATION.md` explains how to run the probe inside Ableton.

## What The Probe Reads

The script walks `live_set` and attempts to read:

- tracks and track names
- selected track
- Session View clip slots and clips
- clip names
- whether clips report as audio or MIDI clips
- audio clip `file_path` when Live exposes it
- device names
- device `class_name`, `class_display_name`, and `type`
- exposed device parameters

The output uses:

```json
{
  "event_type": "ableton_session_probe",
  "proof_level": "directly_observed_via_live_api"
}
```

That proof level means the data was directly observed through Live API metadata. It does not mean audio contribution, authorship, sample license, routing into the capture plugin, or final-export inclusion was proven.

## Research Questions

### Can we list tracks?

Expected yes. The Live Object Model exposes `live_set tracks` as a child list, and the probe counts and scans that list.

### Can we list clips?

Expected partially. The probe scans Session View `clip_slots` on each track and reads the contained `clip` child when `has_clip` is true. Arrangement clips are not included in this first script.

### Can we read clip names?

Expected yes when a clip object is accessible. The probe reads the clip `name` property.

### Can we list devices?

Expected yes. Tracks expose a `devices` child list, and the probe reads each device up to a conservative scan cap.

### Can we distinguish Ableton-native devices vs third-party plugins?

Expected partially. The probe records `class_name`; Live reports third-party plugin devices as `PluginDevice` in current Live Object Model documentation. That distinction is useful, but it does not reveal the plugin's internal preset state or processing history.

### Can we read exposed device parameters?

Expected partially. The Live Object Model exposes automatable parameters through a device `parameters` child list. The probe reads parameter name, value, display value, min, max, and enabled state where available. This does not mean every internal plugin parameter is observable.

### Can we access audio clip source file paths?

Expected yes for accessible audio clip objects if Live exposes the `file_path` property. The probe tries to read `file_path` only after `is_audio_clip` reports true, and records `source_file_accessible: false` when no path is returned.

### Can we observe changes in real time?

Expected partially. `start_observing` registers LiveAPI observers for `live_set tracks` and `live_set view selected_track`, then reruns the probe when those observed values change. This is enough to test real-time feasibility, not enough to claim complete DAW event capture.

### What proof level should this data receive?

Use `directly_observed_via_live_api` for fields returned by the Live API. Use `inferred` for any association between Live metadata and sample watcher/plugin activity. Use `unknown_unobserved` for clip source details, device internals, plugin preset state, routing, or final-export contribution that the probe cannot observe.

## Current Limitations

- This is a human-run Ableton/Max for Live probe, not a daemon integration.
- It only scans Session View clip slots in the first version.
- It does not prove that an observed clip or device contributed to rendered audio.
- It does not inspect third-party plugin internals.
- It does not verify sample licenses or upstream provenance.
- It does not write to the project evidence store yet.
- Scan caps are intentional to avoid locking up a large Live Set during research.

## References

- Ableton: Controlling Live using Max for Live
  <https://help.ableton.com/hc/en-us/articles/5402681764242-Controlling-Live-using-Max-for-Live>
- Cycling '74: LiveAPI JavaScript API
  <https://docs.cycling74.com/apiref/js/liveapi/>
- Cycling '74: Live Object Model
  <https://docs.cycling74.com/apiref/lom/>
