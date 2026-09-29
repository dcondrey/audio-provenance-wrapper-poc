//! REAPER `.rpp`: a port of `daemon/project_formats/reaper.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use apw_core::sha256_hex;

use super::num::{py_float, py_int_hex, py_int_of_float};
use super::safe::{is_python_space, read_project_bytes, Limits, Result};
use super::snapshot::{finish_snapshot, ClipInfo, NeutralProject, ProjectSnapshot, TrackInfo};

const FX_TAGS: [&str; 8] = ["VST", "VST3", "JS", "AU", "AUI", "CLAP", "DX", "VIDEO_EFFECT"];

#[derive(Debug, Default)]
struct RppNode {
    tag: String,
    args: Vec<String>,
    attrs: Vec<(String, Vec<String>)>,
    children: Vec<RppNode>,
    digest: String,
}

impl RppNode {
    fn attr(&self, key: &str) -> Option<&Vec<String>> {
        self.attrs.iter().find(|(name, _)| name == key).map(|(_, values)| values)
    }

    fn descendants<'a>(&'a self, tag: &str, out: &mut Vec<&'a RppNode>) {
        for child in &self.children {
            if child.tag == tag {
                out.push(child);
            }
            child.descendants(tag, out);
        }
    }

    fn walk<'a>(&'a self, out: &mut Vec<&'a RppNode>) {
        out.push(self);
        for child in &self.children {
            child.walk(out);
        }
    }
}

/// Python `str.splitlines()`.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut iter = text.char_indices().peekable();
    while let Some((index, c)) = iter.next() {
        if matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            lines.push(text.get(start..index).unwrap_or_default());
            let mut end = index + c.len_utf8();
            if c == '\r' && bytes.get(end) == Some(&b'\n') {
                iter.next();
                end += 1;
            }
            start = end;
        }
    }
    if start < text.len() {
        lines.push(text.get(start..).unwrap_or_default());
    }
    lines
}

fn py_strip(text: &str) -> &str {
    text.trim_matches(is_python_space)
}

/// Split on whitespace, honouring "double", 'single' and `backtick` quoting.
fn split_args(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&ch) = chars.get(i) {
        if is_python_space(ch) {
            i += 1;
        } else if ch == '"' || ch == '\'' || ch == '`' {
            let rest = chars.get(i + 1..).unwrap_or_default();
            match rest.iter().position(|c| *c == ch) {
                None => {
                    out.push(rest.iter().collect());
                    break;
                }
                Some(offset) => {
                    out.push(rest.get(..offset).unwrap_or_default().iter().collect());
                    i += offset + 2;
                }
            }
        } else {
            let length = chars.get(i..).unwrap_or_default().iter().take_while(|c| !is_python_space(**c)).count();
            out.push(chars.get(i..i + length).unwrap_or_default().iter().collect());
            i += length;
        }
    }
    out
}

fn parse_rpp(text: &str, limits: &Limits) -> Result<RppNode> {
    let lines = splitlines(text);
    if lines.len() > limits.max_rpp_lines {
        return Err("RPP has too many lines; refusing to parse".to_owned());
    }
    let mut root: Option<RppNode> = None;
    let mut stack: Vec<(RppNode, usize)> = Vec::new();
    let mut node_count = 0;
    let close = |node: &mut RppNode, start: usize, end: usize| {
        let joined = lines
            .get(start..=end)
            .unwrap_or_default()
            .iter()
            .map(|line| py_strip(line))
            .collect::<Vec<_>>()
            .join("\n");
        node.digest = sha256_hex(joined.as_bytes()).get(..16).unwrap_or_default().to_owned();
    };
    for (index, raw) in lines.iter().enumerate() {
        let line = py_strip(raw);
        if line.is_empty() {
            continue;
        }
        if root.is_none() && stack.is_empty() && !line.starts_with("<REAPER_PROJECT") {
            return Err("not a REAPER project: missing <REAPER_PROJECT header".to_owned());
        }
        if let Some(rest) = line.strip_prefix('<') {
            let mut parts = split_args(rest);
            if parts.is_empty() {
                return Err(format!("line {}: empty block tag", index + 1));
            }
            let tag = parts.remove(0);
            node_count += 1;
            if node_count > limits.max_rpp_nodes {
                return Err("RPP has too many blocks; refusing to parse".to_owned());
            }
            if stack.is_empty() && root.is_some() {
                return Err(format!("line {}: second top-level block", index + 1));
            }
            if stack.len() >= limits.max_rpp_depth {
                return Err(format!("RPP nesting deeper than {}; refusing to parse", limits.max_rpp_depth));
            }
            stack.push((RppNode { tag, args: parts, ..RppNode::default() }, index));
        } else if line == ">" {
            let Some((mut node, start)) = stack.pop() else {
                return Err(format!("line {}: unbalanced '>'", index + 1));
            };
            close(&mut node, start, index);
            match stack.last_mut() {
                Some((parent, _)) => parent.children.push(node),
                None => root = Some(node),
            }
        } else if let Some((open, _)) = stack.last_mut() {
            // Opaque payload lines (base64 plug-in state) are long and not keys.
            let key_len = match line.find(' ') {
                Some(byte) => line.get(..byte).unwrap_or_default().chars().count(),
                None => line.chars().count(),
            };
            if key_len <= 40 {
                let mut parts = split_args(line);
                if parts.is_empty() {
                    continue;
                }
                let key = parts.remove(0);
                open.attrs.push((key, parts));
            }
        } else {
            return Err(format!("line {}: content outside the project block", index + 1));
        }
    }
    match root {
        Some(root) if stack.is_empty() => Ok(root),
        _ => Err("RPP is empty or has an unclosed block".to_owned()),
    }
}

fn float(values: Option<&Vec<String>>, position: usize, default: f64) -> f64 {
    values.and_then(|v| v.get(position)).and_then(|text| py_float(text)).unwrap_or(default)
}

/// `_int`: `int(float(values[position]))`; an infinity aborts the parse.
fn int(values: Option<&Vec<String>>, position: usize) -> Result<Option<i64>> {
    match values.and_then(|v| v.get(position)) {
        None => Ok(None),
        Some(text) => py_int_of_float(text),
    }
}

fn is_note_on(values: &[String]) -> bool {
    if values.len() < 4 {
        return false;
    }
    let (Some(status), Some(velocity)) = (
        values.get(1).and_then(|text| py_int_hex(text)),
        values.get(3).and_then(|text| py_int_hex(text)),
    ) else {
        return false;
    };
    (status & 0xF0) == 0x90 && velocity > 0
}

struct Source {
    source_type: String,
    file: String,
}

struct Item {
    item_id: String,
    name: String,
    position_seconds: f64,
    length_seconds: f64,
    sources: Vec<Source>,
    digest: String,
}

struct Track {
    track_id: String,
    name: String,
    is_folder: bool,
    items: Vec<Item>,
    devices: Vec<String>,
    fx_digests: BTreeSet<String>,
    envelope_points: usize,
    midi_note_ons: usize,
}

fn extract_track(node: &RppNode, position: usize) -> Result<Track> {
    let mut item_nodes = Vec::new();
    node.descendants("ITEM", &mut item_nodes);
    let mut items = Vec::new();
    for (index, item) in item_nodes.into_iter().enumerate() {
        let mut source_nodes = Vec::new();
        item.descendants("SOURCE", &mut source_nodes);
        let sources = source_nodes
            .into_iter()
            .map(|source| Source {
                source_type: source.args.first().cloned().unwrap_or_default(),
                file: source.attr("FILE").and_then(|values| values.first()).cloned().unwrap_or_default(),
            })
            .collect();
        items.push(Item {
            item_id: item
                .attr("IGUID")
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_else(|| format!("pos-{index}")),
            name: item.attr("NAME").and_then(|values| values.first()).cloned().unwrap_or_default(),
            position_seconds: float(item.attr("POSITION"), 0, 0.0),
            length_seconds: float(item.attr("LENGTH"), 0, 0.0),
            sources,
            digest: item.digest.clone(),
        });
    }
    let mut devices = Vec::new();
    let mut fx_digests = BTreeSet::new();
    let mut chains = Vec::new();
    node.descendants("FXCHAIN", &mut chains);
    for chain in chains {
        fx_digests.insert(chain.digest.clone());
        for child in chain.children.iter().filter(|child| FX_TAGS.contains(&child.tag.as_str())) {
            devices.push(child.args.first().cloned().unwrap_or_else(|| child.tag.clone()));
        }
    }
    let (mut points, mut notes) = (0, 0);
    let mut all = Vec::new();
    node.walk(&mut all);
    for sub in all {
        if sub.tag.contains("ENV") {
            points += sub.attrs.iter().filter(|(key, _)| key == "PT").count();
        }
        if sub.tag == "SOURCE" && sub.args.first().map(String::as_str) == Some("MIDI") {
            notes += sub.attrs.iter().filter(|(key, values)| (key == "E" || key == "e") && is_note_on(values)).count();
        }
    }
    let isbus = node.attr("ISBUS");
    Ok(Track {
        track_id: node.args.first().cloned().unwrap_or_else(|| format!("pos-{position}")),
        name: node.attr("NAME").and_then(|values| values.first()).cloned().unwrap_or_default(),
        is_folder: isbus.is_some_and(|values| !values.is_empty()) && int(isbus, 0)? == Some(1),
        items,
        devices,
        fx_digests,
        envelope_points: points,
        midi_note_ons: notes,
    })
}

pub fn extract_reaper_snapshot_from_text(text: &str, source: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let root = parse_rpp(text, limits)?;
    if root.tag != "REAPER_PROJECT" {
        return Err("not a REAPER project".to_owned());
    }
    let rate = int(root.attr("SAMPLERATE"), 0)?.unwrap_or(0);
    let tempo = root.attr("TEMPO").filter(|values| !values.is_empty());
    let (mut markers, mut regions) = (0, 0);
    for (key, values) in &root.attrs {
        if key == "MARKER" {
            let flags = int(Some(values), 3)?.unwrap_or(0);
            if flags & 1 != 0 {
                regions += 1;
            } else {
                markers += 1;
            }
        }
    }
    let _ = regions;
    let tempo_bpm = tempo.map(|values| float(Some(values), 0, 0.0));
    let time_signature = match tempo {
        Some(values) if values.len() >= 3 => {
            let numerator = int(Some(values), 1)?.filter(|v| *v != 0).unwrap_or(4);
            let denominator = int(Some(values), 2)?.filter(|v| *v != 0).unwrap_or(4);
            Some((numerator, denominator))
        }
        _ => None,
    };
    let tempo_envelope = root.children.iter().any(|child| child.tag.starts_with("TEMPOENV"));
    let _ = tempo_envelope;
    let loop_on = int(root.attr("LOOP"), 0)? == Some(1);
    let mut tracks = Vec::new();
    for (index, child) in root.children.iter().filter(|c| c.tag == "TRACK").enumerate() {
        tracks.push(extract_track(child, index)?);
    }

    let bpm = tempo_bpm.filter(|value| *value != 0.0).unwrap_or(120.0);
    let beats_per_second = bpm / 60.0;
    let mut infos = Vec::new();
    let mut clip_hashes = BTreeSet::new();
    let mut slot_hashes = BTreeMap::new();
    let mut device_hashes = BTreeSet::new();
    let mut sample_refs = BTreeSet::new();
    for track in &tracks {
        let mut clips = Vec::new();
        let mut track_samples: Vec<String> = Vec::new();
        for item in &track.items {
            clip_hashes.insert(item.digest.clone());
            slot_hashes.insert((track.track_id.clone(), item.item_id.clone()), item.digest.clone());
            let files: Vec<&String> = item.sources.iter().map(|s| &s.file).filter(|f| !f.is_empty()).collect();
            for file in &files {
                sample_refs.insert((*file).clone());
                if !track_samples.contains(file) {
                    track_samples.push((*file).clone());
                }
            }
            let is_midi = !item.sources.is_empty() && item.sources.iter().all(|s| s.source_type == "MIDI");
            clips.push(ClipInfo {
                name: item.name.clone(),
                position_beats: item.position_seconds * beats_per_second,
                length_beats: item.length_seconds * beats_per_second,
                sample_ref: files.first().map(|f| (*f).clone()).unwrap_or_default(),
                warp_on: false,
                is_midi,
            });
        }
        device_hashes.extend(track.fx_digests.iter().cloned());
        infos.push(TrackInfo {
            track_id: track.track_id.clone(),
            name: track.name.clone(),
            track_type: if track.is_folder { "FolderTrack" } else { "Track" }.to_owned(),
            devices: track.devices.clone(),
            device_presets: vec![String::new(); track.devices.len()],
            sample_paths: track_samples,
            clip_count: track.items.len(),
            clips,
            automation_point_count: track.envelope_points,
            midi_note_count: track.midi_note_ons,
            device_chain_hashes: track.fx_digests.clone(),
            group_id: String::new(),
            routing_input: String::new(),
            routing_output: String::new(),
            sends: Vec::new(),
            is_frozen: false,
            color_index: -1,
        });
    }
    let mut project = NeutralProject::new("reaper_rpp", Vec::new());
    project.bpm = bpm;
    project.time_signature = time_signature.unwrap_or((4, 4));
    project.loop_on = loop_on;
    project.loop_range = (0.0, 0.0);
    project.locators = markers;
    project.sample_rate = if rate > 0 { Some(rate) } else { None };
    finish_snapshot(infos, clip_hashes, slot_hashes, device_hashes, sample_refs, &project, source)
}

pub fn extract_reaper_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let bytes = read_project_bytes(path, limits)?;
    extract_reaper_snapshot_from_text(&String::from_utf8_lossy(&bytes), path, limits)
}
