//! DAWproject (`.dawproject`): a port of `daemon/project_formats/dawproject.py`.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use super::num::{py_float, py_int};
use super::safe::{check_member_name, open_zip, parse_xml, read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, canonical_element, digest_of, NeutralClip, NeutralProject, NeutralTrack, ProjectSnapshot};
use super::xml::Element;
use apw_core::sha256_hex;

const PROJECT_MEMBER: &str = "project.xml";
const POINT_TAGS: [&str; 6] = ["Point", "RealPoint", "BoolPoint", "EnumPoint", "IntegerPoint", "TimeSignaturePoint"];

fn float(value: Option<&str>, default: f64) -> f64 {
    value.and_then(py_float).unwrap_or(default)
}

fn int(value: Option<&str>, default: i64) -> i64 {
    value
        .and_then(py_int)
        .map_or(default, |value| i64::try_from(value).unwrap_or(default))
}

fn unit<'a>(element: &'a Element, inherited: &'a str) -> &'a str {
    match element.get("timeUnit") {
        Some(found @ ("beats" | "seconds")) => found,
        _ => inherited,
    }
}

fn to_beats(value: f64, unit: &str, bpm: f64) -> f64 {
    if unit == "seconds" {
        value * bpm / 60.0
    } else {
        value
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.filter(|text| !text.is_empty())
}

fn device_label(device: &Element) -> String {
    let name = nonempty(device.get("deviceName")).or_else(|| nonempty(device.get("name"))).unwrap_or("");
    format!("{}: {name}", device.tag)
}

fn role_type(role: &str) -> &'static str {
    match role {
        "master" => "MasterTrack",
        "effect" => "ReturnTrack",
        "submix" => "SubmixTrack",
        "vca" => "VcaTrack",
        _ => "Track",
    }
}

struct Content {
    files: Vec<String>,
    notes: usize,
    has_notes: bool,
}

fn clip_content(clip: &Element) -> Content {
    let mut files = Vec::new();
    let mut notes = 0;
    let mut has_notes_tag = false;
    for element in clip.iter() {
        match element.tag.as_str() {
            "Audio" => {
                if let Some(path) = element.child("File").and_then(|file| nonempty(file.get("path"))) {
                    files.push(path.to_owned());
                }
            }
            "Note" => notes += 1,
            "Notes" => has_notes_tag = true,
            _ => {}
        }
    }
    Content { files, notes, has_notes: notes > 0 || has_notes_tag }
}

fn walk_clips<'a>(node: &'a Element, node_unit: &'a str, out: &mut Vec<(&'a Element, &'a str)>) {
    for child in &node.children {
        if child.tag == "Clip" {
            out.push((child, node_unit));
        } else {
            walk_clips(child, unit(child, node_unit), out);
        }
    }
}

fn arrangement_clips(lane: &Element, lane_unit: &str, bpm: f64) -> (Vec<NeutralClip>, usize, usize) {
    let points = lane.iter().iter().filter(|element| POINT_TAGS.contains(&element.tag.as_str())).count();
    let mut ordered = Vec::new();
    walk_clips(lane, lane_unit, &mut ordered);
    let mut clips = Vec::new();
    let mut notes = 0;
    for (index, (clip, clip_unit)) in ordered.into_iter().enumerate() {
        let content = clip_content(clip);
        notes += content.notes;
        clips.push(NeutralClip {
            clip_id: format!("clip-{index}"),
            name: clip.get_or("name", "").to_owned(),
            position_beats: to_beats(float(clip.get("time"), 0.0), clip_unit, bpm),
            length_beats: to_beats(float(clip.get("duration"), 0.0), clip_unit, bpm),
            sample_ref: content.files.first().cloned().unwrap_or_default(),
            is_midi: content.has_notes && content.files.is_empty(),
            warp_on: clip.iter().iter().any(|element| element.tag == "Warps"),
            digest: digest_of(&[canonical_element(clip, &["id"])]),
        });
    }
    (clips, points, notes)
}

fn collect_tracks<'a>(
    parent: &'a Element,
    group: &str,
    depth: usize,
    limit: usize,
    out: &mut Vec<(&'a Element, String)>,
) -> Result<()> {
    if depth > limit {
        return Err(format!("DAWproject track nesting deeper than {limit}; refusing to parse"));
    }
    for track in parent.children_named("Track") {
        out.push((track, group.to_owned()));
        collect_tracks(track, track.get_or("id", ""), depth + 1, limit, out)?;
    }
    Ok(())
}

pub fn extract_dawproject(data: &[u8], limits: &Limits) -> Result<NeutralProject> {
    let mut archive = open_zip(data, limits)?;
    if !archive.names.iter().any(|name| name == PROJECT_MEMBER) {
        return Err("not a DAWproject: archive has no project.xml".to_owned());
    }
    let xml = archive.read_member(PROJECT_MEMBER, limits.max_xml_bytes as u64)?;
    let root = parse_xml(&xml, None, limits)?;
    if root.tag != "Project" {
        return Err("not a DAWproject: project.xml root is not <Project>".to_owned());
    }

    let transport = root.child("Transport");
    let tempo = transport.and_then(|t| t.child("Tempo"));
    let signature = transport.and_then(|t| t.child("TimeSignature"));
    let bpm = match tempo {
        Some(tempo) if tempo.get("unit") == Some("bpm") => float(tempo.get("value"), 0.0),
        _ => 0.0,
    };
    let time_signature = (
        signature.map_or(4, |s| int(s.get("numerator"), 4)),
        signature.map_or(4, |s| int(s.get("denominator"), 4)),
    );

    let mut flat: Vec<(&Element, String)> = Vec::new();
    if let Some(structure) = root.child("Structure") {
        collect_tracks(structure, "", 0, limits.max_dawproject_track_depth, &mut flat)?;
    }

    let arrangement = root.child("Arrangement");
    let mut lanes_by_track: Vec<(String, Vec<(&Element, &str)>)> = Vec::new();
    let mut markers = 0;
    if let Some(arrangement) = arrangement {
        markers = arrangement.iter_tag("Marker").len();
        let mut stack: Vec<(&Element, &str)> = arrangement
            .children_named("Lanes")
            .map(|lane| (lane, unit(lane, "beats")))
            .collect();
        while let Some((lane, lane_unit)) = stack.pop() {
            match nonempty(lane.get("track")) {
                Some(reference) => match lanes_by_track.iter_mut().find(|(key, _)| key == reference) {
                    Some((_, lanes)) => lanes.push((lane, lane_unit)),
                    None => lanes_by_track.push((reference.to_owned(), vec![(lane, lane_unit)])),
                },
                None => stack.extend(lane.children_named("Lanes").map(|child| (child, unit(child, lane_unit)))),
            }
        }
    }

    let mut tracks = Vec::new();
    for (position, (track, group)) in flat.iter().enumerate() {
        let track_id = nonempty(track.get("id")).map_or_else(|| format!("pos-{position}"), str::to_owned);
        let channel = track.child("Channel");
        let role = channel.map_or("regular", |c| c.get_or("role", "regular"));
        let mut devices = Vec::new();
        let mut presets = Vec::new();
        let mut hashes: Vec<Value> = Vec::new();
        if let Some(container) = channel.and_then(|c| c.child("Devices")) {
            for device in &container.children {
                devices.push(device_label(device));
                let state = state_digest(&mut archive, device, limits)?;
                hashes.push(Value::String(digest_of(&[canonical_element(device, &["id"]), json!(state)])));
                presets.push(state);
            }
        }
        let mut clips: Vec<NeutralClip> = Vec::new();
        let (mut points, mut notes) = (0, 0);
        let wanted = track.get_or("id", "");
        if let Some((_, lanes)) = lanes_by_track.iter().find(|(key, _)| key == wanted) {
            for (lane, lane_unit) in lanes {
                let (lane_clips, lane_points, lane_notes) = arrangement_clips(lane, lane_unit, bpm);
                for mut clip in lane_clips {
                    clip.clip_id = format!("clip-{}", clips.len());
                    clips.push(clip);
                }
                points += lane_points;
                notes += lane_notes;
            }
        }
        let mut neutral = NeutralTrack::new(track_id, track.get_or("name", "").to_owned());
        neutral.track_type = if track.child("Track").is_some() { "FolderTrack" } else { role_type(role) }.to_owned();
        neutral.devices = devices;
        neutral.device_presets = presets;
        neutral.device_chain_hashes = if hashes.is_empty() {
            BTreeSet::new()
        } else {
            BTreeSet::from([digest_of(&[Value::Array(hashes)])])
        };
        neutral.clips = clips;
        neutral.automation_points = points;
        neutral.midi_notes = notes;
        neutral.group_id = group.clone();
        tracks.push(neutral);
    }

    let mut project = NeutralProject::new("dawproject", tracks);
    project.bpm = bpm;
    project.time_signature = time_signature;
    project.locators = markers;
    Ok(project)
}

fn state_digest(archive: &mut super::safe::Archive, device: &Element, limits: &Limits) -> Result<String> {
    let Some(state) = device.child("State") else {
        return Ok(String::new());
    };
    let Some(path) = nonempty(state.get("path")) else {
        return Ok(String::new());
    };
    if state.get("external") == Some("true") || !archive.names.iter().any(|name| name == path) {
        return Ok(String::new());
    }
    check_member_name(path)?;
    let bytes = archive.read_member(path, limits.max_project_file_bytes)?;
    Ok(sha256_hex(&bytes).get(..16).unwrap_or_default().to_owned())
}

pub fn extract_dawproject_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let data = read_project_bytes(path, limits)?;
    build_snapshot(&extract_dawproject(&data, limits)?, path)
}
