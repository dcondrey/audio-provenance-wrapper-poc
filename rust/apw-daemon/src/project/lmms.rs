//! LMMS projects (`.mmp` / `.mmpz`): a port of `daemon/project_formats/lmms.py`.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::Value;

use super::num::{py_float, py_int_of_float};
use super::safe::{inflate_zlib, parse_xml, read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, canonical_element, digest_of, NeutralClip, NeutralProject, NeutralTrack, ProjectSnapshot};
use super::xml::Element;

const TICKS_PER_BEAT: f64 = 48.0;
const LMMS_DOCTYPE: &str = "lmms-project";
const MIDI_CLIPS: [&str; 2] = ["midiclip", "pattern"];
const SAMPLE_CLIPS: [&str; 2] = ["sampleclip", "sampletco"];
const AUTOMATION_CLIPS: [&str; 2] = ["automationclip", "automationpattern"];
const PATTERN_CLIPS: [&str; 1] = ["bbtco"];
const SETTINGS: [&str; 5] = ["instrumenttrack", "sampletrack", "patterntrack", "bbtrack", "automationtrack"];

fn track_type(kind: i64) -> &'static str {
    match kind {
        0 => "InstrumentTrack",
        1 => "PatternTrack",
        2 => "SampleTrack",
        3 => "EventTrack",
        4 => "VideoTrack",
        5 => "AutomationTrack",
        6 => "HiddenAutomationTrack",
        _ => "Track",
    }
}

/// `_int`: `int(float(value))`, with an infinity aborting the parse.
fn int(value: Option<&str>, default: i64) -> Result<i64> {
    match value {
        None => Ok(default),
        Some(text) => Ok(py_int_of_float(text)?.unwrap_or(default)),
    }
}

fn float(value: Option<&str>) -> f64 {
    value.and_then(py_float).unwrap_or(0.0)
}

/// Plain XML first, then qUncompress, mirroring `DataFile::loadData`.
fn load_lmms_xml(data: &[u8], limits: &Limits) -> Result<Element> {
    let stripped = {
        let start = data.iter().position(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)).unwrap_or(data.len());
        data.get(start..).unwrap_or_default()
    };
    if stripped.starts_with(b"<") {
        return parse_xml(data, Some(LMMS_DOCTYPE), limits);
    }
    if data.len() < 5 {
        return Err("not an LMMS project".to_owned());
    }
    let header: [u8; 4] = data.get(..4).and_then(|bytes| bytes.try_into().ok()).unwrap_or_default();
    let declared = u64::from(u32::from_be_bytes(header));
    if declared > limits.max_xml_bytes as u64 {
        return Err(format!(
            "LMMS project declares {declared} bytes, past {}; refusing to parse",
            limits.max_xml_bytes
        ));
    }
    let inflated = inflate_zlib(data.get(4..).unwrap_or_default(), declared.min(limits.max_xml_bytes as u64))?;
    if inflated.len() as u64 != declared {
        return Err("LMMS compressed length does not match its header".to_owned());
    }
    parse_xml(&inflated, Some(LMMS_DOCTYPE), limits)
}

fn head_value<'a>(head: Option<&'a Element>, name: &str) -> Option<&'a str> {
    let head = head?;
    if head.get(name).is_some() {
        return head.get(name);
    }
    head.child(name).and_then(|child| child.get("value"))
}

fn clip_for(element: &Element, index: usize) -> Result<(NeutralClip, usize, usize)> {
    let notes = element.children_named("note").count();
    let points = element.children_named("time").count();
    let tag = element.tag.as_str();
    let sample = if SAMPLE_CLIPS.contains(&tag) { element.get_or("src", "") } else { "" };
    let clip = NeutralClip {
        clip_id: format!("clip-{index}"),
        name: element.get_or("name", "").to_owned(),
        position_beats: int(element.get("pos"), 0)? as f64 / TICKS_PER_BEAT,
        length_beats: int(element.get("len"), 0)? as f64 / TICKS_PER_BEAT,
        sample_ref: sample.to_owned(),
        is_midi: MIDI_CLIPS.contains(&tag),
        warp_on: false,
        digest: digest_of(&[canonical_element(element, &[])]),
    };
    Ok((clip, notes, points))
}

fn track(element: &Element, position: usize) -> Result<NeutralTrack> {
    let kind = int(element.get("type"), -1)?;
    let settings = element.children.iter().find(|child| SETTINGS.contains(&child.tag.as_str()));
    let mut devices = Vec::new();
    let mut hashes: Vec<Value> = Vec::new();
    let mut instrument_samples = Vec::new();
    if let Some(settings) = settings {
        if let Some(instrument) = settings.child("instrument") {
            devices.push(format!("instrument: {}", instrument.get_or("name", "")));
            hashes.push(Value::String(digest_of(&[canonical_element(instrument, &[])])));
            if let Some(src) = instrument
                .child("audiofileprocessor")
                .and_then(|sampler| sampler.get("src"))
                .filter(|src| !src.is_empty())
            {
                instrument_samples.push(src.to_owned());
            }
        }
        if let Some(chain) = settings.child("fxchain") {
            for effect in chain.children_named("effect") {
                devices.push(format!("effect: {}", effect.get_or("name", "")));
                hashes.push(Value::String(digest_of(&[canonical_element(effect, &[])])));
            }
        }
    }
    let mut clips = Vec::new();
    let (mut note_total, mut point_total) = (0, 0);
    for child in &element.children {
        let tag = child.tag.as_str();
        if MIDI_CLIPS.contains(&tag) || SAMPLE_CLIPS.contains(&tag) || AUTOMATION_CLIPS.contains(&tag) || PATTERN_CLIPS.contains(&tag) {
            let (clip, notes, points) = clip_for(child, clips.len())?;
            clips.push(clip);
            note_total += notes;
            point_total += points;
        }
    }
    let mut neutral = NeutralTrack::new(format!("pos-{position}"), element.get_or("name", "").to_owned());
    neutral.track_type = track_type(kind).to_owned();
    neutral.devices = devices;
    neutral.device_chain_hashes = if hashes.is_empty() {
        BTreeSet::new()
    } else {
        BTreeSet::from([digest_of(&[Value::Array(hashes)])])
    };
    neutral.clips = clips;
    neutral.extra_sample_refs = instrument_samples;
    neutral.automation_points = point_total;
    neutral.midi_notes = note_total;
    Ok(neutral)
}

pub fn extract_lmms(data: &[u8], limits: &Limits) -> Result<NeutralProject> {
    let root = load_lmms_xml(data, limits)?;
    if root.tag != "lmms-project" {
        return Err("not an LMMS project: root element is not <lmms-project>".to_owned());
    }
    if !matches!(root.get("type"), None | Some("song")) {
        return Err(format!(
            "unsupported LMMS file type {}; only song projects are read",
            apw_core::python_repr(&Value::String(root.get_or("type", "").to_owned()))
        ));
    }
    let head = root.child("head");
    let bpm = float(head_value(head, "bpm"));
    let numerator = int(head_value(head, "timesig_numerator"), 4)?;
    let denominator = int(head_value(head, "timesig_denominator"), 4)?;
    let signature = (if numerator == 0 { 4 } else { numerator }, if denominator == 0 { 4 } else { denominator });

    let mut tracks = Vec::new();
    let mut loop_on = false;
    let mut loop_range = (0.0, 0.0);
    if let Some(song) = root.child("song") {
        for (index, element) in song.iter_tag("track").into_iter().enumerate() {
            tracks.push(track(element, index)?);
        }
        if let Some(timeline) = song.child("timeline") {
            loop_on = int(timeline.get("lpstate"), 0)? == 1;
            loop_range = (
                int(timeline.get("lp0pos"), 0)? as f64 / TICKS_PER_BEAT,
                int(timeline.get("lp1pos"), 0)? as f64 / TICKS_PER_BEAT,
            );
        }
    }
    let mut project = NeutralProject::new("lmms", tracks);
    project.bpm = bpm;
    project.time_signature = signature;
    project.loop_on = loop_on;
    project.loop_range = loop_range;
    Ok(project)
}

pub fn extract_lmms_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let data = read_project_bytes(path, limits)?;
    build_snapshot(&extract_lmms(&data, limits)?, path)
}
