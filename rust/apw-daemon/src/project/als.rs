//! Ableton Live sets (`.als`): a port of `extract_snapshot` in
//! `daemon/project_differ/differ.py`.
//!
//! The clip-slot and device-chain hashes are SHA-256 over the ElementTree
//! serialisation of each subtree, so [`super::xml::Element::to_xml_string`]
//! reproduces `ET.tostring` (tail text included, non-ASCII as character
//! references). XML namespaces are not supported.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use apw_core::sha256_hex;

use super::num::{py_float, py_int};
use super::safe::{Limits, Result};
use super::snapshot::{finish_snapshot, ClipInfo, NeutralProject, ProjectSnapshot, SendInfo, TrackInfo};
use super::xml::{parse_xml, Element, XmlLimits};

fn short(text: &str) -> String {
    sha256_hex(text.as_bytes()).get(..16).unwrap_or_default().to_owned()
}

/// Decompress and parse an Ableton `.als` file. Refuses oversized and
/// DTD-carrying files.
pub fn parse_als(path: &Path, limits: &Limits) -> Result<Element> {
    let file = std::fs::File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let cap = limits.max_als_decompressed_bytes;
    let mut data = Vec::new();
    flate2::read::MultiGzDecoder::new(file)
        .take(cap + 1)
        .read_to_end(&mut data)
        .map_err(|error| format!("{} is not a readable gzip file: {error}", path.display()))?;
    if data.len() as u64 > cap {
        return Err(format!("{} decompresses past {cap} bytes; refusing to parse", path.display()));
    }
    if data.windows(9).any(|window| window == b"<!DOCTYPE") {
        return Err(format!("{} contains a DTD declaration; refusing to parse", path.display()));
    }
    parse_xml(
        &data,
        None,
        XmlLimits { max_bytes: usize::MAX, max_depth: limits.max_als_depth, max_elements: usize::MAX, et_mode: true },
    )
}

fn attr(element: Option<&Element>, name: &str, default: &str) -> String {
    element.and_then(|e| e.get(name)).unwrap_or(default).to_owned()
}

/// `float(text)` that aborts the parse on garbage, like Python's `ValueError`.
fn strict_float(text: &str) -> Result<f64> {
    py_float(text).ok_or_else(|| format!("could not convert string to float: {}", apw_core::python_repr(&serde_json::Value::String(text.to_owned()))))
}

pub fn extract_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let root = parse_als(path, limits)?;
    let Some(live_set) = root.child("LiveSet") else {
        return Err(format!("No LiveSet element in {}", path.display()));
    };

    let mut track_names: Vec<String> = Vec::new();
    let mut clip_hashes = BTreeSet::new();
    let mut device_hashes = BTreeSet::new();
    let mut sample_refs = BTreeSet::new();
    let mut infos: Vec<TrackInfo> = Vec::new();
    let mut clip_slot_hashes: BTreeMap<(String, String), String> = BTreeMap::new();

    if let Some(tracks_el) = live_set.child("Tracks") {
        for (track_index, track) in tracks_el.children.iter().enumerate() {
            let track_id = track
                .get("Id")
                .filter(|text| !text.is_empty())
                .map_or_else(|| format!("pos-{track_index}"), str::to_owned);
            let name_val = attr(track.child("Name").and_then(|n| n.child("EffectiveName")), "Value", "");
            track_names.push(name_val.clone());

            let mut track_clips = 0;
            let mut track_devices = Vec::new();
            let mut track_presets = Vec::new();
            let mut track_samples: Vec<String> = Vec::new();
            let mut track_clip_infos = Vec::new();
            let mut track_device_hashes = BTreeSet::new();

            for (slot_index, clip_slot) in track.iter_tag("ClipSlot").into_iter().enumerate() {
                track_clips += 1;
                let slot_hash = short(&clip_slot.to_xml_string());
                clip_hashes.insert(slot_hash.clone());
                let slot_id = clip_slot
                    .get("Id")
                    .filter(|text| !text.is_empty())
                    .map_or_else(|| format!("pos-{slot_index}"), str::to_owned);
                clip_slot_hashes.insert((track_id.clone(), slot_id), slot_hash);

                let audio = clip_slot.find(".//AudioClip");
                let clip_el = audio.or_else(|| clip_slot.find(".//MidiClip"));
                if let Some(clip_el) = clip_el {
                    let clip_name = attr(clip_el.child("Name"), "Value", "");
                    let pos = match clip_el.child("CurrentStart") {
                        Some(el) => strict_float(el.get_or("Value", "0"))?,
                        None => 0.0,
                    };
                    let end = match clip_el.child("CurrentEnd") {
                        Some(el) => strict_float(el.get_or("Value", "0"))?,
                        None => 0.0,
                    };
                    let warp_on = clip_el.find(".//WarpMode").is_some();
                    let clip_sample = attr(clip_el.find(".//FileRef/Path"), "Value", "");
                    track_clip_infos.push(ClipInfo {
                        name: clip_name,
                        position_beats: pos,
                        length_beats: end - pos,
                        sample_ref: clip_sample,
                        warp_on,
                        is_midi: clip_el.tag == "MidiClip",
                    });
                }
            }

            for device_chain in track.iter_tag("DeviceChain") {
                let chain_hash = short(&device_chain.to_xml_string());
                device_hashes.insert(chain_hash.clone());
                track_device_hashes.insert(chain_hash);
                if let Some(devices_el) = device_chain.find(".//Devices") {
                    for device in &devices_el.children {
                        let mut dev_name = device.tag.clone();
                        if let Some(user) = device.find(".//UserName").and_then(|el| el.get("Value")).filter(|v| !v.is_empty()) {
                            dev_name = user.to_owned();
                        }
                        track_devices.push(dev_name);
                        track_presets.push(attr(device.find(".//SelectedPresetName"), "Value", ""));
                    }
                }
            }

            for file_ref in track.iter_tag("FileRef") {
                if let Some(value) = file_ref.child("RelativePath").map(|rel| rel.get_or("Value", "")).filter(|v| !v.is_empty()) {
                    sample_refs.insert(value.to_owned());
                    track_samples.push(value.to_owned());
                }
                if let Some(value) = file_ref.child("Path").map(|abs| abs.get_or("Value", "")).filter(|v| !v.is_empty()) {
                    sample_refs.insert(value.to_owned());
                    if !track_samples.iter().any(|existing| existing == value) {
                        track_samples.push(value.to_owned());
                    }
                }
            }

            let mut track_midi = 0;
            for notes_el in track.iter_tag("Notes") {
                for key_track in notes_el.iter_tag("KeyTrack") {
                    track_midi += key_track.iter_tag("MidiNoteEvent").len();
                }
            }
            let track_auto = track.iter_tag("AutomationPoint").len();

            let group_id = attr(track.child("TrackGroupId"), "Value", "");
            let routing_in = attr(track.find(".//AudioInputRouting/Target"), "Value", "");
            let routing_out = attr(track.find(".//AudioOutputRouting/Target"), "Value", "");

            let mut sends = Vec::new();
            for holder in track.iter_tag("TrackSendHolder") {
                let send_target = attr(holder.find(".//Send/Target"), "Value", "");
                let send_level = holder
                    .find(".//Send/Manual")
                    .and_then(|el| py_float(el.get_or("Value", "0")))
                    .unwrap_or(0.0);
                if !send_target.is_empty() {
                    sends.push(SendInfo { target: send_target, level: send_level });
                }
            }
            let frozen = track.child("Freeze").is_some_and(|el| el.get_or("Value", "false") == "true");
            let color_index = track
                .child("ColorIndex")
                .and_then(|el| py_int(el.get_or("Value", "-1")))
                .and_then(|value| i64::try_from(value).ok())
                .unwrap_or(-1);

            infos.push(TrackInfo {
                track_id,
                name: name_val,
                track_type: track.tag.clone(),
                devices: track_devices,
                device_presets: track_presets,
                sample_paths: track_samples,
                clips: track_clip_infos,
                clip_count: track_clips,
                automation_point_count: track_auto,
                midi_note_count: track_midi,
                device_chain_hashes: track_device_hashes,
                group_id,
                routing_input: routing_in,
                routing_output: routing_out,
                sends,
                is_frozen: frozen,
                color_index,
            });
        }
    }

    let mut bpm = 120.0;
    let mut loop_on = false;
    let mut loop_start = 0.0;
    let mut loop_length = 0.0;
    if let Some(transport) = live_set.child("Transport") {
        if let Some(tempo) = transport.find(".//Tempo/Manual") {
            bpm = strict_float(tempo.get_or("Value", "120"))?;
        }
        if let Some(el) = transport.child("LoopOn") {
            loop_on = el.get_or("Value", "false") == "true";
        }
        if let Some(el) = transport.child("LoopStart") {
            loop_start = strict_float(el.get_or("Value", "0"))?;
        }
        if let Some(el) = transport.child("LoopLength") {
            loop_length = strict_float(el.get_or("Value", "0"))?;
        }
    }
    let mut signature = (4_i64, 4_i64);
    if let Some(ts) = live_set.find(".//TimeSignature") {
        if let Some(num) = ts.find(".//Numerator").and_then(|el| py_int(el.get_or("Value", "4"))) {
            signature.0 = i64::try_from(num).unwrap_or(4);
        }
        if let Some(den) = ts.find(".//Denominator").and_then(|el| py_int(el.get_or("Value", "4"))) {
            signature.1 = i64::try_from(den).unwrap_or(4);
        }
    }
    let locator_count = live_set.iter_tag("Locator").len();

    let mut project = NeutralProject::new("ableton_als", Vec::new());
    project.bpm = bpm;
    project.time_signature = signature;
    project.loop_on = loop_on;
    project.loop_range = (loop_start, loop_length);
    project.locators = locator_count;
    let mut snapshot = finish_snapshot(infos, clip_hashes, clip_slot_hashes, device_hashes, sample_refs, &project, path)?;
    snapshot.track_names = track_names;
    Ok(snapshot)
}
