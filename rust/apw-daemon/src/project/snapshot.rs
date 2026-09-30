//! The project snapshot and the format-neutral model the parsers fill: a port of
//! `daemon/project_formats/_snapshot.py` and the `ProjectSnapshot` dataclasses of
//! `daemon/project_differ/differ.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use apw_core::{python_repr_f64, sha256_file, sha256_hex};
use serde_json::{json, Value};

use super::xml::{py_strip, Element};

#[derive(Debug, Clone, PartialEq)]
pub struct ClipInfo {
    pub name: String,
    pub position_beats: f64,
    pub length_beats: f64,
    pub sample_ref: String,
    pub warp_on: bool,
    pub is_midi: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SendInfo {
    pub target: String,
    pub level: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackInfo {
    pub track_id: String,
    pub name: String,
    pub track_type: String,
    pub devices: Vec<String>,
    pub device_presets: Vec<String>,
    pub sample_paths: Vec<String>,
    pub clips: Vec<ClipInfo>,
    pub clip_count: usize,
    pub automation_point_count: usize,
    pub midi_note_count: usize,
    pub device_chain_hashes: BTreeSet<String>,
    pub group_id: String,
    pub routing_input: String,
    pub routing_output: String,
    pub sends: Vec<SendInfo>,
    pub is_frozen: bool,
    pub color_index: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectSnapshot {
    pub file_hash: String,
    pub file_size_bytes: u64,
    pub track_count: usize,
    pub track_names: Vec<String>,
    pub tracks: Vec<TrackInfo>,
    pub clip_count: usize,
    pub clip_hashes: BTreeSet<String>,
    pub clip_slot_hashes: BTreeMap<(String, String), String>,
    pub device_chain_hashes: BTreeSet<String>,
    pub automation_point_count: usize,
    pub midi_note_count: usize,
    pub sample_refs: BTreeSet<String>,
    pub transport_bpm: f64,
    pub transport_time_signature: (i64, i64),
    pub transport_loop_on: bool,
    pub transport_loop_range: (f64, f64),
    pub locator_count: usize,
    pub sample_rate: Option<i64>,
    pub project_format: String,
}

/// A JSON number rendered the way Python's `json.dumps` renders the float.
pub fn float_value(value: f64) -> Value {
    python_repr_f64(value)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null)
}

/// `json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)`.
/// Written out here because the shared canonicaliser bounds nesting, while the
/// canonical form of a deeply nested XML subtree is legitimately hundreds of
/// levels deep. Recursion is bounded by the parsers' depth limits.
pub fn py_json_dumps(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            let text = number.to_string();
            // Python parses "-0" to the int 0 and prints "0".
            let text = if text == "-0" { "0".to_owned() } else { text };
            if text.contains(['.', 'e', 'E']) {
                out.push_str(&number.as_f64().and_then(|f| python_repr_f64(f).ok()).unwrap_or(text));
            } else {
                out.push_str(&text);
            }
        }
        Value::String(text) => out.push_str(&serde_json::to_string(text).unwrap_or_default()),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                py_json_dumps(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_default());
                out.push(':');
                py_json_dumps(item, out);
            }
            out.push('}');
        }
    }
}

/// Stable 16-hex fingerprint of JSON-serialisable parts (`digest_of`).
pub fn digest_of(parts: &[Value]) -> String {
    let mut blob = String::new();
    py_json_dumps(&Value::Array(parts.to_vec()), &mut blob);
    sha256_hex(blob.as_bytes()).get(..16).unwrap_or_default().to_owned()
}

/// Iterative canonical form of an XML subtree: tag, sorted attributes, stripped
/// text, children (`canonical_element`).
pub fn canonical_element(element: &Element, ignore_attrs: &[&str]) -> Value {
    fn build(node: &Element, ignore: &[&str]) -> Value {
        let mut attrs: Vec<(&str, &str)> = node
            .attrs
            .iter()
            .filter(|(key, _)| !ignore.contains(&key.as_str()))
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        attrs.sort_unstable();
        let attrs: Vec<Value> = attrs.into_iter().map(|(key, value)| json!([key, value])).collect();
        let children: Vec<Value> = node.children.iter().map(|child| build(child, ignore)).collect();
        json!([node.tag, attrs, py_strip(&node.text_all()), children])
    }
    // Depth is bounded by the parser's limit.
    build(element, ignore_attrs)
}

#[derive(Debug, Clone, Default)]
pub struct NeutralClip {
    pub clip_id: String,
    pub name: String,
    pub position_beats: f64,
    pub length_beats: f64,
    pub sample_ref: String,
    pub is_midi: bool,
    pub warp_on: bool,
    pub digest: String,
}

#[derive(Debug, Clone)]
pub struct NeutralTrack {
    pub track_id: String,
    pub name: String,
    pub track_type: String,
    pub devices: Vec<String>,
    pub device_presets: Vec<String>,
    pub device_chain_hashes: BTreeSet<String>,
    pub clips: Vec<NeutralClip>,
    pub extra_sample_refs: Vec<String>,
    pub automation_points: usize,
    pub midi_notes: usize,
    pub group_id: String,
}

impl NeutralTrack {
    pub fn new(track_id: String, name: String) -> Self {
        NeutralTrack {
            track_id,
            name,
            track_type: "Track".to_owned(),
            devices: Vec::new(),
            device_presets: Vec::new(),
            device_chain_hashes: BTreeSet::new(),
            clips: Vec::new(),
            extra_sample_refs: Vec::new(),
            automation_points: 0,
            midi_notes: 0,
            group_id: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct NeutralProject {
    pub project_format: &'static str,
    pub tracks: Vec<NeutralTrack>,
    pub bpm: f64,
    pub time_signature: (i64, i64),
    pub loop_on: bool,
    pub loop_range: (f64, f64),
    pub locators: usize,
    pub sample_rate: Option<i64>,
}

impl NeutralProject {
    pub fn new(project_format: &'static str, tracks: Vec<NeutralTrack>) -> Self {
        NeutralProject {
            project_format,
            tracks,
            bpm: 0.0,
            time_signature: (4, 4),
            loop_on: false,
            loop_range: (0.0, 0.0),
            locators: 0,
            sample_rate: None,
        }
    }
}

pub fn build_snapshot(project: &NeutralProject, source: &Path) -> Result<ProjectSnapshot, String> {
    let mut clip_hashes = BTreeSet::new();
    let mut slot_hashes = BTreeMap::new();
    let mut device_hashes = BTreeSet::new();
    let mut sample_refs = BTreeSet::new();
    let mut infos: Vec<TrackInfo> = Vec::new();

    for track in &project.tracks {
        let mut clips = Vec::new();
        let mut track_samples: Vec<String> = Vec::new();
        for clip in &track.clips {
            clip_hashes.insert(clip.digest.clone());
            slot_hashes.insert((track.track_id.clone(), clip.clip_id.clone()), clip.digest.clone());
            if !clip.sample_ref.is_empty() && !track_samples.contains(&clip.sample_ref) {
                track_samples.push(clip.sample_ref.clone());
            }
            clips.push(ClipInfo {
                name: clip.name.clone(),
                position_beats: clip.position_beats,
                length_beats: clip.length_beats,
                sample_ref: clip.sample_ref.clone(),
                warp_on: clip.warp_on,
                is_midi: clip.is_midi,
            });
        }
        for reference in &track.extra_sample_refs {
            if !track_samples.contains(reference) {
                track_samples.push(reference.clone());
            }
        }
        sample_refs.extend(track_samples.iter().cloned());
        device_hashes.extend(track.device_chain_hashes.iter().cloned());
        let presets = if track.device_presets.is_empty() {
            vec![String::new(); track.devices.len()]
        } else {
            track.device_presets.clone()
        };
        infos.push(TrackInfo {
            track_id: track.track_id.clone(),
            name: track.name.clone(),
            track_type: track.track_type.clone(),
            devices: track.devices.clone(),
            device_presets: presets,
            sample_paths: track_samples,
            clip_count: clips.len(),
            clips,
            automation_point_count: track.automation_points,
            midi_note_count: track.midi_notes,
            device_chain_hashes: track.device_chain_hashes.clone(),
            group_id: track.group_id.clone(),
            routing_input: String::new(),
            routing_output: String::new(),
            sends: Vec::new(),
            is_frozen: false,
            color_index: -1,
        });
    }
    finish_snapshot(infos, clip_hashes, slot_hashes, device_hashes, sample_refs, project, source)
}

/// Shared tail of every parser: totals plus the file hash and size.
pub fn finish_snapshot(
    infos: Vec<TrackInfo>,
    clip_hashes: BTreeSet<String>,
    clip_slot_hashes: BTreeMap<(String, String), String>,
    device_chain_hashes: BTreeSet<String>,
    sample_refs: BTreeSet<String>,
    project: &NeutralProject,
    source: &Path,
) -> Result<ProjectSnapshot, String> {
    let file_hash = sha256_file(source).map_err(|error| error.to_string())?;
    let file_size_bytes = std::fs::metadata(source)
        .map_err(|error| format!("cannot stat {}: {error}", source.display()))?
        .len();
    Ok(ProjectSnapshot {
        file_hash,
        file_size_bytes,
        track_count: infos.len(),
        track_names: infos.iter().map(|track| track.name.clone()).collect(),
        clip_count: infos.iter().map(|track| track.clip_count).sum(),
        automation_point_count: infos.iter().map(|track| track.automation_point_count).sum(),
        midi_note_count: infos.iter().map(|track| track.midi_note_count).sum(),
        tracks: infos,
        clip_hashes,
        clip_slot_hashes,
        device_chain_hashes,
        sample_refs,
        transport_bpm: project.bpm,
        transport_time_signature: project.time_signature,
        transport_loop_on: project.loop_on,
        transport_loop_range: project.loop_range,
        locator_count: project.locators,
        sample_rate: project.sample_rate,
        project_format: project.project_format.to_owned(),
    })
}

fn clip_golden(clip: &ClipInfo) -> Value {
    json!({
        "name": clip.name,
        "position_beats": float_value(clip.position_beats),
        "length_beats": float_value(clip.length_beats),
        "sample_ref": clip.sample_ref,
        "warp_on": clip.warp_on,
        "is_midi": clip.is_midi,
    })
}

fn track_golden(track: &TrackInfo) -> Value {
    json!({
        "track_id": track.track_id,
        "name": track.name,
        "track_type": track.track_type,
        "devices": track.devices,
        "device_presets": track.device_presets,
        "sample_paths": track.sample_paths,
        "clips": track.clips.iter().map(clip_golden).collect::<Vec<_>>(),
        "clip_count": track.clip_count,
        "automation_point_count": track.automation_point_count,
        "midi_note_count": track.midi_note_count,
        "device_chain_hashes": track.device_chain_hashes.iter().collect::<Vec<_>>(),
        "group_id": track.group_id,
    })
}

/// The deterministic, path-free JSON form committed beside each fixture
/// (`snapshot_to_golden`).
pub fn snapshot_to_golden(snapshot: &ProjectSnapshot) -> Value {
    json!({
        "project_format": snapshot.project_format,
        "file_hash": snapshot.file_hash,
        "file_size_bytes": snapshot.file_size_bytes,
        "track_count": snapshot.track_count,
        "track_names": snapshot.track_names,
        "tracks": snapshot.tracks.iter().map(track_golden).collect::<Vec<_>>(),
        "clip_count": snapshot.clip_count,
        "clip_hashes": snapshot.clip_hashes.iter().collect::<Vec<_>>(),
        "clip_slot_hashes": snapshot
            .clip_slot_hashes
            .iter()
            .map(|((track, clip), hash)| json!([track, clip, hash]))
            .collect::<Vec<_>>(),
        "device_chain_hashes": snapshot.device_chain_hashes.iter().collect::<Vec<_>>(),
        "automation_point_count": snapshot.automation_point_count,
        "midi_note_count": snapshot.midi_note_count,
        "sample_refs": snapshot.sample_refs.iter().collect::<Vec<_>>(),
        "transport_bpm": float_value(snapshot.transport_bpm),
        "transport_time_signature": [snapshot.transport_time_signature.0, snapshot.transport_time_signature.1],
        "transport_loop_on": snapshot.transport_loop_on,
        "transport_loop_range": [float_value(snapshot.transport_loop_range.0), float_value(snapshot.transport_loop_range.1)],
        "locator_count": snapshot.locator_count,
        "sample_rate": snapshot.sample_rate,
    })
}
