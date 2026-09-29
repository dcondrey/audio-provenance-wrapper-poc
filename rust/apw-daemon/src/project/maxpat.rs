//! Max patchers (`.maxpat`): a port of `daemon/project_formats/maxpat.py`.

use std::collections::BTreeSet;
use std::path::Path;

use apw_core::python_str;
use serde_json::{json, Map, Value};

use super::safe::{parse_json, read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, digest_of, NeutralProject, NeutralTrack, ProjectSnapshot};

const AUDIO_EXTENSIONS: [&str; 8] = [".wav", ".aif", ".aiff", ".flac", ".ogg", ".mp3", ".w64", ".caf"];
const LAYOUT_KEYS: [&str; 8] = [
    "patching_rect", "presentation_rect", "rect", "presentation", "fontsize", "fontname", "fontface", "bgcolor",
];

/// Drop layout keys and nested patchers (they are their own tracks).
fn strip(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map
                .iter()
                .filter(|(key, _)| !LAYOUT_KEYS.contains(&key.as_str()) && key.as_str() != "patcher")
                .collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Object(entries.into_iter().map(|(key, child)| (key.clone(), strip(child))).collect::<Map<_, _>>())
        }
        Value::Array(items) => Value::Array(items.iter().map(strip).collect()),
        other => other.clone(),
    }
}

fn box_label(box_: &Map<String, Value>) -> Option<String> {
    let maxclass = box_.get("maxclass").unwrap_or(&Value::Null);
    let text = match box_.get("text") {
        Some(Value::String(text)) => text.as_str(),
        _ => "",
    };
    if maxclass.as_str() == Some("comment") {
        return None;
    }
    if maxclass.as_str() == Some("newobj") {
        return Some(text.to_owned());
    }
    Some(if text.is_empty() {
        python_str(maxclass)
    } else {
        format!("{}: {text}", python_str(maxclass))
    })
}

pub fn extract_maxpat(data: &[u8], limits: &Limits) -> Result<NeutralProject> {
    let document = parse_json(data, limits)?;
    let Some(Value::Object(root)) = document.get("patcher").filter(|_| document.is_object()) else {
        return Err("not a Max patcher: no top-level 'patcher' object".to_owned());
    };
    let mut tracks: Vec<NeutralTrack> = Vec::new();
    type Pending<'a> = (&'a Map<String, Value>, String, &'static str, String);
    let mut pending: Vec<Pending> = vec![(root, "main".to_owned(), "Patch", String::new())];
    while let Some((patcher, name, kind, parent)) = pending.pop() {
        if tracks.len() >= limits.max_maxpat_patchers {
            return Err(format!("Max patch has more than {} patchers; refusing to parse", limits.max_maxpat_patchers));
        }
        let track_id = format!("patcher-{}", tracks.len());
        let mut devices = Vec::new();
        let mut samples: Vec<String> = Vec::new();
        let mut stripped_boxes: Vec<Value> = Vec::new();
        let mut children: Vec<Pending> = Vec::new();
        if let Some(Value::Array(entries)) = patcher.get("boxes") {
            for entry in entries {
                let Some(Value::Object(box_)) = entry.get("box").filter(|_| entry.is_object()) else {
                    continue;
                };
                stripped_boxes.push(strip(&Value::Object(box_.clone())));
                if let Some(label) = box_label(box_) {
                    samples.extend(label.split(super::safe::is_python_space).filter(|atom| !atom.is_empty()).filter(|atom| {
                        let lower = atom.to_lowercase();
                        AUDIO_EXTENSIONS.iter().any(|extension| lower.ends_with(extension))
                    }).map(str::to_owned));
                    devices.push(label);
                }
                if let Some(Value::Object(nested)) = box_.get("patcher") {
                    let text = match box_.get("text") {
                        Some(Value::String(text)) => text.clone(),
                        _ => String::new(),
                    };
                    let display = if text.is_empty() {
                        box_.get("id").map(python_str).unwrap_or_default()
                    } else {
                        text
                    };
                    children.push((nested, display, "Subpatch", track_id.clone()));
                }
            }
        }
        let lines = match patcher.get("lines") {
            Some(list @ Value::Array(_)) => strip(list),
            _ => json!([]),
        };
        let mut track = NeutralTrack::new(track_id, name);
        track.track_type = kind.to_owned();
        track.devices = devices;
        track.device_chain_hashes = BTreeSet::from([digest_of(&[Value::Array(stripped_boxes), lines])]);
        let mut seen: Vec<String> = Vec::new();
        for sample in samples {
            if !seen.contains(&sample) {
                seen.push(sample);
            }
        }
        track.extra_sample_refs = seen;
        track.group_id = parent;
        tracks.push(track);
        pending.extend(children.into_iter().rev());
    }
    Ok(NeutralProject::new("max_patcher", tracks))
}

pub fn extract_maxpat_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let data = read_project_bytes(path, limits)?;
    build_snapshot(&extract_maxpat(&data, limits)?, path)
}
