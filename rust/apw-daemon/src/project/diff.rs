//! Structural diff between two project snapshots: a port of `compute_diff`,
//! `ProjectDiff` and `diff_to_event` in `daemon/project_differ/differ.py`.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::snapshot::{float_value, ProjectSnapshot, TrackInfo};

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectDiff {
    pub timestamp_ms: i64,
    pub previous_file_hash: String,
    pub current_file_hash: String,
    pub tracks_added: Vec<String>,
    pub tracks_removed: Vec<String>,
    pub clips_added: usize,
    pub clips_removed: usize,
    pub clips_modified: usize,
    pub devices_changed: Vec<String>,
    pub automation_points_delta: i64,
    pub midi_notes_delta: i64,
    pub samples_added: BTreeSet<String>,
    pub samples_removed: BTreeSet<String>,
    pub bpm_changed: bool,
    pub loop_changed: bool,
    pub locators_delta: i64,
}

impl ProjectDiff {
    pub fn has_changes(&self) -> bool {
        !self.tracks_added.is_empty()
            || !self.tracks_removed.is_empty()
            || self.clips_added > 0
            || self.clips_removed > 0
            || self.clips_modified > 0
            || !self.devices_changed.is_empty()
            || self.automation_points_delta != 0
            || self.midi_notes_delta != 0
            || !self.samples_added.is_empty()
            || !self.samples_removed.is_empty()
            || self.bpm_changed
            || self.loop_changed
            || self.locators_delta != 0
    }
}

fn by_id(snapshot: &ProjectSnapshot) -> BTreeMap<&str, &TrackInfo> {
    // A later track with the same id replaces an earlier one, like a Python dict.
    snapshot.tracks.iter().map(|track| (track.track_id.as_str(), track)).collect()
}

fn delta(current: usize, previous: usize) -> i64 {
    i64::try_from(current).unwrap_or(i64::MAX).saturating_sub(i64::try_from(previous).unwrap_or(i64::MAX))
}

pub fn compute_diff(previous: &ProjectSnapshot, current: &ProjectSnapshot, timestamp_ms: i64) -> ProjectDiff {
    let previous_tracks = by_id(previous);
    let current_tracks = by_id(current);
    let previous_slots = &previous.clip_slot_hashes;
    let current_slots = &current.clip_slot_hashes;
    let clips_modified = current_slots
        .iter()
        .filter(|(key, hash)| previous_slots.get(*key).is_some_and(|earlier| earlier != *hash))
        .count();
    let mut devices_changed: Vec<String> = current_tracks
        .iter()
        .filter(|(id, track)| {
            previous_tracks
                .get(*id)
                .is_some_and(|earlier| earlier.device_chain_hashes != track.device_chain_hashes)
        })
        .map(|(_, track)| track.name.clone())
        .collect();
    devices_changed.sort();
    let mut tracks_added: Vec<String> = current_tracks
        .iter()
        .filter(|(id, _)| !previous_tracks.contains_key(*id))
        .map(|(_, track)| track.name.clone())
        .collect();
    tracks_added.sort();
    let mut tracks_removed: Vec<String> = previous_tracks
        .iter()
        .filter(|(id, _)| !current_tracks.contains_key(*id))
        .map(|(_, track)| track.name.clone())
        .collect();
    tracks_removed.sort();
    ProjectDiff {
        timestamp_ms,
        previous_file_hash: previous.file_hash.clone(),
        current_file_hash: current.file_hash.clone(),
        tracks_added,
        tracks_removed,
        clips_added: current_slots.keys().filter(|key| !previous_slots.contains_key(*key)).count(),
        clips_removed: previous_slots.keys().filter(|key| !current_slots.contains_key(*key)).count(),
        clips_modified,
        devices_changed,
        automation_points_delta: delta(current.automation_point_count, previous.automation_point_count),
        midi_notes_delta: delta(current.midi_note_count, previous.midi_note_count),
        samples_added: current.sample_refs.difference(&previous.sample_refs).cloned().collect(),
        samples_removed: previous.sample_refs.difference(&current.sample_refs).cloned().collect(),
        bpm_changed: current.transport_bpm != previous.transport_bpm,
        loop_changed: current.transport_loop_on != previous.transport_loop_on
            || current.transport_loop_range != previous.transport_loop_range,
        locators_delta: delta(current.locator_count, previous.locator_count),
    }
}

/// The `project_diff` evidence event both daemons write.
pub fn diff_to_event(diff: &ProjectDiff, monotonic_ms: i64) -> Map<String, Value> {
    let value = json!({
        "event_type": "project_diff",
        "proof_level": "inferred",
        "source_timestamp_ms": diff.timestamp_ms,
        "timestamp_ms": diff.timestamp_ms,
        "daemon_observed_monotonic_ms": monotonic_ms,
        "clips_added": diff.clips_added,
        "clips_removed": diff.clips_removed,
        "clips_modified": diff.clips_modified,
        "tracks_added": diff.tracks_added,
        "tracks_removed": diff.tracks_removed,
        "devices_changed": diff.devices_changed,
        "samples_added": diff.samples_added.iter().collect::<Vec<_>>(),
        "samples_removed": diff.samples_removed.iter().collect::<Vec<_>>(),
        "midi_notes_delta": diff.midi_notes_delta,
        "automation_points_delta": diff.automation_points_delta,
        "bpm_changed": diff.bpm_changed,
    });
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

/// The `session_facts` manifest section built from the latest snapshot.
pub fn session_facts(snapshot: &ProjectSnapshot) -> Value {
    let tracks: Vec<Value> = snapshot
        .tracks
        .iter()
        .map(|track| {
            json!({
                "name": track.name,
                "type": track.track_type,
                "devices": track.devices,
                "device_presets": track.device_presets,
                "sample_paths": track.sample_paths,
                "clips": track.clips.iter().map(|clip| json!({
                    "name": clip.name,
                    "position_beats": float_value(clip.position_beats),
                    "length_beats": float_value(clip.length_beats),
                    "sample_ref": clip.sample_ref,
                    "warp_on": clip.warp_on,
                    "is_midi": clip.is_midi,
                })).collect::<Vec<_>>(),
                "clip_count": track.clip_count,
                "midi_note_count": track.midi_note_count,
                "automation_point_count": track.automation_point_count,
                "routing_input": track.routing_input,
                "routing_output": track.routing_output,
                "sends": track.sends.iter().map(|send| json!({"target": send.target, "level": float_value(send.level)})).collect::<Vec<_>>(),
                "group_id": track.group_id,
                "is_frozen": track.is_frozen,
                "color_index": track.color_index,
            })
        })
        .collect();
    json!({
        "apw:proof_level": "inferred",
        "observation_basis": "Structural facts inferred by parsing the saved Ableton .als file; \
Ableton does not provide this project with a supported semantic API.",
        "bpm": float_value(snapshot.transport_bpm),
        "time_signature": format!("{}/{}", snapshot.transport_time_signature.0, snapshot.transport_time_signature.1),
        "loop_on": snapshot.transport_loop_on,
        "track_count": snapshot.track_count,
        "clip_count": snapshot.clip_count,
        "sample_refs": snapshot.sample_refs.iter().collect::<Vec<_>>(),
        "tracks": tracks,
    })
}
