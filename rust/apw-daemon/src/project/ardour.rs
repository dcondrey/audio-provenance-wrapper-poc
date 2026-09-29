//! Ardour sessions (`.ardour`): a port of `daemon/project_formats/ardour.py`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::{json, Value};

use super::num::{py_float, py_int};
use super::safe::{parse_xml, read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, canonical_element, digest_of, NeutralClip, NeutralProject, NeutralTrack, ProjectSnapshot};
use super::xml::{py_strip, Element};

const TICKS_PER_BEAT: f64 = 1920.0;

fn int(value: Option<&str>) -> Option<i128> {
    value.and_then(py_int)
}

fn float(value: Option<&str>) -> Option<f64> {
    value.and_then(py_float)
}

struct Clock {
    superclock_rate: Option<i128>,
    sample_rate: Option<i128>,
    bpm: f64,
}

/// `^([ab])(-?\d+)$` on stripped text.
fn split_timepos(text: &str) -> Option<(char, i128)> {
    let mut chars = text.chars();
    let kind = chars.next().filter(|c| *c == 'a' || *c == 'b')?;
    let digits = chars.as_str();
    let body = digits.strip_prefix('-').unwrap_or(digits);
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<i128>().ok().map(|amount| (kind, amount))
}

impl Clock {
    fn beats(&self, text: Option<&str>) -> f64 {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return 0.0;
        };
        let text = py_strip(text);
        if let Some((kind, amount)) = split_timepos(text) {
            if kind == 'b' {
                return amount as f64 / TICKS_PER_BEAT;
            }
            return match self.superclock_rate {
                Some(rate) => amount as f64 / rate as f64 * self.bpm / 60.0,
                None => 0.0,
            };
        }
        match (int(Some(text)), self.sample_rate) {
            (Some(legacy), Some(rate)) => legacy as f64 / rate as f64 * self.bpm / 60.0,
            _ => 0.0,
        }
    }

    fn length_and_position(&self, text: Option<&str>) -> (f64, f64) {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            return (0.0, 0.0);
        };
        let (distance, position) = text.split_once('@').unwrap_or((text, ""));
        (self.beats(Some(distance)), self.beats(Some(position)))
    }
}

fn tempo(root: &Element) -> (f64, (i64, i64), Option<i128>) {
    let Some(map) = root.child("TempoMap") else {
        return (0.0, (4, 4), None);
    };
    let rate = int(map.get("superclocks-per-second"));
    let mut bpm = 0.0;
    if let Some(tempo) = map.find("Tempos/Tempo") {
        let npm = float(tempo.get("npm")).filter(|v| *v != 0.0).unwrap_or(0.0);
        let note_type = float(tempo.get("note-type")).filter(|v| *v != 0.0).unwrap_or(4.0);
        bpm = if note_type > 0.0 { npm * 4.0 / note_type } else { 0.0 };
    }
    let mut signature = (4, 4);
    if let Some(meter) = map.find("Meters/Meter") {
        let per_bar = float(meter.get("divisions-per-bar"));
        let note_value = float(meter.get("note-value"));
        if let (Some(per_bar), Some(note_value)) = (per_bar, note_value) {
            if per_bar != 0.0 && note_value != 0.0 && per_bar > 0.0 && note_value > 0.0 {
                signature = (per_bar.trunc() as i64, note_value.trunc() as i64);
            }
        }
    }
    (bpm, signature, rate.filter(|rate| *rate > 0))
}

fn region_clips(playlist: &Element, sources: &BTreeMap<String, String>, clock: &Clock) -> Vec<NeutralClip> {
    let mut clips = Vec::new();
    for (index, region) in playlist.children_named("Region").enumerate() {
        let (length, mut position) = clock.length_and_position(region.get("length"));
        let first_is_digit = region
            .get_or("length", "")
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit());
        if first_is_digit && region.get("position").is_some_and(|text| !text.is_empty()) {
            position = clock.beats(region.get("position"));
        }
        let source_id = region.get_or("source-0", "");
        let type_ = region
            .get("type")
            .filter(|text| !text.is_empty())
            .or_else(|| playlist.get("type").filter(|text| !text.is_empty()))
            .unwrap_or("");
        clips.push(NeutralClip {
            clip_id: region
                .get("id")
                .filter(|text| !text.is_empty())
                .map_or_else(|| format!("pos-{index}"), str::to_owned),
            name: region.get_or("name", "").to_owned(),
            position_beats: position,
            length_beats: length,
            sample_ref: sources.get(source_id).cloned().unwrap_or_default(),
            is_midi: type_ == "midi",
            warp_on: false,
            digest: digest_of(&[canonical_element(region, &["id"])]),
        });
    }
    clips
}

pub fn extract_ardour(data: &[u8], limits: &Limits) -> Result<NeutralProject> {
    if data.starts_with(&[0x1f, 0x8b]) {
        return Err("compressed .ardour files are not supported".to_owned());
    }
    let root = parse_xml(data, None, limits)?;
    if root.tag != "Session" {
        return Err("not an Ardour session: root element is not <Session>".to_owned());
    }
    let sample_rate = int(root.get("sample-rate"));
    let (bpm, signature, rate) = tempo(&root);
    let clock = Clock { superclock_rate: rate, sample_rate: sample_rate.filter(|rate| *rate > 0), bpm };

    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    for source in root.find_all("Sources/Source") {
        let id = source.get("id").filter(|text| !text.is_empty());
        if let (Some(id), true) = (id, matches!(source.get("type"), Some("audio") | None)) {
            sources.insert(id.to_owned(), source.get_or("name", "").to_owned());
        }
    }
    let mut playlists: BTreeMap<&str, &Element> = BTreeMap::new();
    for playlist in root.find_all("Playlists/Playlist") {
        if let Some(id) = playlist.get("id").filter(|text| !text.is_empty()) {
            playlists.insert(id, playlist);
        }
    }

    let mut tracks = Vec::new();
    for (position, route) in root.find_all("Routes/Route").into_iter().enumerate() {
        let flags = route.child("PresentationInfo").map_or("", |info| info.get_or("flags", ""));
        let playlist_ids = [route.get("audio-playlist"), route.get("midi-playlist")];
        let is_track = playlist_ids.iter().any(|id| id.is_some_and(|text| !text.is_empty()));
        let mut clips = Vec::new();
        for id in playlist_ids.iter().flatten().filter(|text| !text.is_empty()) {
            if let Some(playlist) = playlists.get(id) {
                clips.extend(region_clips(playlist, &sources, &clock));
            }
        }
        let mut devices = Vec::new();
        let mut hashes: Vec<Value> = Vec::new();
        for processor in route.children_named("Processor") {
            if let Some(unique_id) = processor.get("unique-id").filter(|text| !text.is_empty()) {
                devices.push(format!("{}: {}", processor.get_or("type", ""), processor.get_or("name", "")));
                hashes.push(Value::String(digest_of(&[
                    json!(processor.get("type")),
                    json!(unique_id),
                    json!(processor.get("active")),
                ])));
            }
        }
        let mut track = NeutralTrack::new(
            route
                .get("id")
                .filter(|text| !text.is_empty())
                .map_or_else(|| format!("pos-{position}"), str::to_owned),
            route.get_or("name", "").to_owned(),
        );
        track.track_type = if flags.is_empty() {
            if is_track { "Track" } else { "Bus" }.to_owned()
        } else {
            flags.to_owned()
        };
        track.devices = devices;
        track.device_chain_hashes = if hashes.is_empty() {
            BTreeSet::new()
        } else {
            BTreeSet::from([digest_of(&[Value::Array(hashes)])])
        };
        track.clips = clips;
        tracks.push(track);
    }

    let mut locators = 0;
    let mut loop_range = (0.0, 0.0);
    let mut loop_on = false;
    for location in root.find_all("Locations/Location") {
        let flags: Vec<&str> = location.get_or("flags", "").split(',').map(py_strip).collect();
        if flags.iter().any(|flag| *flag == "IsMark" || *flag == "IsRangeMarker") {
            locators += 1;
        }
        if flags.contains(&"IsAutoLoop") {
            loop_on = true;
            loop_range = (clock.beats(location.get("start")), clock.beats(location.get("end")));
        }
    }

    let mut project = NeutralProject::new("ardour", tracks);
    project.bpm = bpm;
    project.time_signature = signature;
    project.loop_on = loop_on;
    project.loop_range = loop_range;
    project.locators = locators;
    project.sample_rate = clock.sample_rate.and_then(|rate| i64::try_from(rate).ok());
    Ok(project)
}

pub fn extract_ardour_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let data = read_project_bytes(path, limits)?;
    build_snapshot(&extract_ardour(&data, limits)?, path)
}
