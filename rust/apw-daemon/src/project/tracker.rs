//! MilkyTracker module formats (`.xm`, `.mod`): a port of
//! `daemon/project_formats/tracker.py`; the layout notes and grounding are there.

use std::collections::BTreeSet;
use std::path::Path;

use apw_core::sha256_hex;
use serde_json::{json, Value};

use super::safe::{read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, digest_of, NeutralProject, NeutralTrack, ProjectSnapshot};

const MAX_XM_CHANNELS: u64 = 32;
const MAX_XM_PATTERNS: u64 = 256;
const MAX_XM_INSTRUMENTS: u64 = 255;
const MAX_XM_SAMPLES_PER_INSTRUMENT: u64 = 96;
const MAX_XM_ORDERS: u64 = 256;
const MAX_XM_ROWS: u64 = 256;

const XM_MIN_HEADER: usize = 272;
const XM_VERSION: u64 = 0x0104;

/// Bytes up to the first NUL, trailing spaces removed, decoded as ISO-8859-1.
fn text(raw: &[u8]) -> String {
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    let trimmed = raw.get(..end).unwrap_or_default();
    let stop = trimmed.iter().rposition(|byte| *byte != b' ').map_or(0, |index| index + 1);
    trimmed.get(..stop).unwrap_or_default().iter().map(|byte| char::from(*byte)).collect()
}

fn slice(data: &[u8], start: usize, end: usize) -> &[u8] {
    data.get(start..end).unwrap_or_default()
}

fn le(data: &[u8], start: usize, len: usize) -> u64 {
    slice(data, start, start + len).iter().rev().fold(0_u64, |acc, byte| (acc << 8) | u64::from(*byte))
}

fn u16le(data: &[u8], offset: usize) -> u64 {
    le(data, offset, 2)
}

fn u32le(data: &[u8], offset: usize) -> u64 {
    le(data, offset, 4)
}

fn sha(data: &[u8]) -> String {
    sha256_hex(data)
}

fn sample_device(name: &str, length: u64) -> String {
    format!("{name} ({length} bytes)")
}

fn patterns_track(order: &[u8], patterns: Vec<Value>) -> NeutralTrack {
    let mut track = NeutralTrack::new("patterns".to_owned(), "patterns".to_owned());
    track.track_type = "Patterns".to_owned();
    track.device_chain_hashes = BTreeSet::from([digest_of(&[json!(order), Value::Array(patterns)])]);
    track
}

fn instrument_track(index: usize, name: String, devices: Vec<String>, digest: String) -> NeutralTrack {
    let mut track = NeutralTrack::new(format!("instrument-{index}"), name);
    track.track_type = "Instrument".to_owned();
    track.devices = devices;
    track.device_chain_hashes = BTreeSet::from([digest]);
    track
}

fn module_track(name: String, facts: Vec<String>) -> NeutralTrack {
    let mut track = NeutralTrack::new("module".to_owned(), name);
    track.track_type = "Module".to_owned();
    track.devices = facts;
    track
}

pub fn extract_xm(data: &[u8]) -> Result<NeutralProject> {
    if data.len() < 64 {
        return Err("XM header truncated".to_owned());
    }
    if data.get(..16) != Some(b"Extended Module:".as_slice()) {
        return Err("not an XM module: missing 'Extended Module:' id text".to_owned());
    }
    let version = u16le(data, 58);
    if version != XM_VERSION {
        return Err(format!("unsupported XM version 0x{version:04x}; only 0x0104 is read"));
    }
    let header_size = u32le(data, 60) as usize;
    if header_size < 4 {
        return Err("XM header size is smaller than its own field".to_owned());
    }
    let header_end = 60 + header_size;
    if header_end > data.len() {
        return Err("XM header truncated".to_owned());
    }
    let mut header = slice(data, 64, header_end).to_vec();
    if header.len() < XM_MIN_HEADER {
        header.resize(XM_MIN_HEADER, 0);
    }
    let order_count = u16le(&header, 0).min(MAX_XM_ORDERS);
    let restart = u16le(&header, 2);
    let channels = u16le(&header, 4);
    let pattern_count = u16le(&header, 6);
    let instrument_count = u16le(&header, 8);
    let flags = u16le(&header, 10);
    let ticks = u16le(&header, 12);
    let bpm = u16le(&header, 14);
    let order: Vec<u8> = slice(&header, 16, 272).iter().copied().take(order_count as usize).collect();
    if !(1..=MAX_XM_CHANNELS).contains(&channels) {
        return Err(format!("XM channel count {channels} is outside 1..{MAX_XM_CHANNELS}"));
    }
    if pattern_count > MAX_XM_PATTERNS {
        return Err(format!("XM has {pattern_count} patterns, past {MAX_XM_PATTERNS}"));
    }
    if instrument_count > MAX_XM_INSTRUMENTS {
        return Err(format!("XM has {instrument_count} instruments, past {MAX_XM_INSTRUMENTS}"));
    }

    let mut position = header_end;
    let mut patterns: Vec<Value> = Vec::new();
    for _ in 0..pattern_count {
        if position + 9 > data.len() {
            return Err("XM pattern header truncated".to_owned());
        }
        let length = u32le(data, position) as usize;
        let rows = u16le(data, position + 5);
        let packed = u16le(data, position + 7) as usize;
        if length < 9 {
            return Err("XM pattern header length is smaller than 9".to_owned());
        }
        if !(1..=MAX_XM_ROWS).contains(&rows) {
            return Err(format!("XM pattern has {rows} rows, outside 1..{MAX_XM_ROWS}"));
        }
        let start = position + length;
        if start + packed > data.len() {
            return Err("XM pattern data truncated".to_owned());
        }
        patterns.push(json!([rows, packed, sha(slice(data, start, start + packed))]));
        position = start + packed;
    }

    let mut truncated = false;
    let mut instruments: Vec<NeutralTrack> = Vec::new();
    let mut sample_total = 0_usize;
    for index in 0..instrument_count as usize {
        if position >= data.len() {
            truncated = true;
            break;
        }
        if position + 29 > data.len() {
            return Err("XM instrument header truncated".to_owned());
        }
        let size = u32le(data, position) as usize;
        if size < 29 {
            return Err("XM instrument header size is smaller than 29".to_owned());
        }
        let end = position + size;
        if end > data.len() {
            return Err("XM instrument header truncated".to_owned());
        }
        let name = text(slice(data, position + 4, position + 26));
        let sample_count = u16le(data, position + 27);
        if sample_count > MAX_XM_SAMPLES_PER_INSTRUMENT {
            return Err(format!("XM instrument has {sample_count} samples, past {MAX_XM_SAMPLES_PER_INSTRUMENT}"));
        }
        let mut samples: Vec<(String, u64)> = Vec::new();
        let mut parts: Vec<Value> = vec![json!(sha(slice(data, position, end)))];
        let mut cursor = end;
        if sample_count > 0 {
            if size < 33 {
                return Err("XM instrument header is too small for its samples".to_owned());
            }
            let sample_header = u32le(data, position + 29) as usize;
            if sample_header < 40 {
                return Err("XM sample header size is smaller than 40".to_owned());
            }
            let mut headers: Vec<&[u8]> = Vec::new();
            for _ in 0..sample_count {
                if cursor + sample_header > data.len() {
                    return Err("XM sample header truncated".to_owned());
                }
                headers.push(slice(data, cursor, cursor + sample_header));
                cursor += sample_header;
            }
            for header_bytes in headers {
                let length = u32le(header_bytes, 0);
                let stop = (cursor + length as usize).min(data.len());
                if cursor + length as usize > data.len() {
                    truncated = true;
                }
                samples.push((text(slice(header_bytes, 18, 40)), length));
                parts.push(json!([sha(header_bytes), sha(slice(data, cursor, stop))]));
                cursor = stop;
            }
        }
        sample_total += samples.len();
        instruments.push(instrument_track(
            index + 1,
            name,
            samples.iter().map(|(sample, length)| sample_device(sample, *length)).collect(),
            digest_of(&[Value::Array(parts)]),
        ));
        position = cursor;
    }

    let mut facts = vec![
        format!("format: XM {}.{:02}", version >> 8, version & 0xFF),
        format!("tracker: {}", text(slice(data, 38, 58))),
        format!("channels: {channels}"),
        format!("song length: {order_count}"),
        format!("restart position: {restart}"),
        format!("patterns: {pattern_count}"),
        format!("instruments: {instrument_count}"),
        format!("samples: {sample_total}"),
        format!("frequency table: {}", if flags & 1 == 1 { "linear" } else { "amiga" }),
        format!("ticks per row: {ticks}"),
        format!("bpm: {bpm}"),
    ];
    if truncated {
        facts.push("truncated".to_owned());
    }
    let mut tracks = vec![module_track(text(slice(data, 17, 37)), facts), patterns_track(&order, patterns)];
    tracks.extend(instruments);
    let mut project = NeutralProject::new("milkytracker", tracks);
    project.bpm = bpm as f64;
    Ok(project)
}

fn mod_channels(tag: &[u8]) -> u64 {
    if [b"M.K.".as_slice(), b"M!K!", b"FLT4"].contains(&tag) {
        return 4;
    }
    if [b"FLT8".as_slice(), b"OKTA", b"OCTA", b"FA08", b"CD81"].contains(&tag) {
        return 8;
    }
    let first = tag.first().copied().unwrap_or(0);
    let second = tag.get(1).copied().unwrap_or(0);
    let rest = tag.get(1..).unwrap_or_default();
    if (b'1'..=b'9').contains(&first) && rest == b"CHN" {
        return u64::from(first - b'0');
    }
    if (b'1'..=b'9').contains(&first) && second.is_ascii_digit() && [b"CH".as_slice(), b"CN"].contains(&tag.get(2..).unwrap_or_default()) {
        return u64::from(first - b'0') * 10 + u64::from(second - b'0');
    }
    0
}

pub fn extract_mod(data: &[u8]) -> Result<NeutralProject> {
    if data.len() < 1084 {
        return Err("MOD header truncated".to_owned());
    }
    let tag = slice(data, 1080, 1084);
    let channels = mod_channels(tag) as usize;
    if channels == 0 {
        return Err("not a supported MOD module: no recognised format tag at offset 1080".to_owned());
    }
    let song_length = data.get(950).copied().unwrap_or(0);
    let restart = data.get(951).copied().unwrap_or(0);
    let order: Vec<u8> = slice(data, 952, 1080).to_vec();
    let pattern_count = usize::from(order.iter().copied().max().unwrap_or(0)) + 1;
    let pattern_size = channels * 256;
    let mut truncated = false;
    let mut patterns: Vec<Value> = Vec::new();
    let mut position = 1084_usize;
    for _ in 0..pattern_count {
        let stop = (position + pattern_size).min(data.len());
        if position + pattern_size > data.len() {
            truncated = true;
        }
        patterns.push(json!([64, stop - position, sha(slice(data, position, stop))]));
        position = stop;
    }

    let mut instruments: Vec<NeutralTrack> = Vec::new();
    let mut sample_total = 0_usize;
    for slot in 0..31_usize {
        let offset = 20 + 30 * slot;
        let name = text(slice(data, offset, offset + 22));
        let length = u64::from(u16::from_be_bytes([
            data.get(offset + 22).copied().unwrap_or(0),
            data.get(offset + 23).copied().unwrap_or(0),
        ])) * 2;
        let mut payload: &[u8] = &[];
        if length > 2 {
            let stop = (position + length as usize).min(data.len());
            if position + length as usize > data.len() {
                truncated = true;
            }
            payload = slice(data, position, stop);
            position = stop;
        }
        if name.is_empty() && length == 0 {
            continue;
        }
        if length > 2 {
            sample_total += 1;
        }
        let devices = if length > 2 { vec![sample_device(&name, length)] } else { Vec::new() };
        let digest = digest_of(&[json!([sha(slice(data, offset, offset + 30)), sha(payload)])]);
        instruments.push(instrument_track(slot + 1, name, devices, digest));
    }

    let mut facts = vec![
        format!("format: MOD {}", text(tag)),
        format!("channels: {channels}"),
        format!("song length: {song_length}"),
        format!("restart position: {restart}"),
        format!("patterns: {pattern_count}"),
        format!("samples: {sample_total}"),
    ];
    if truncated {
        facts.push("truncated".to_owned());
    }
    let ordered = order.get(..usize::from(song_length).min(128)).unwrap_or_default();
    let mut tracks = vec![module_track(text(slice(data, 0, 20)), facts), patterns_track(ordered, patterns)];
    tracks.extend(instruments);
    Ok(NeutralProject::new("milkytracker", tracks))
}

pub fn extract_module_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let data = read_project_bytes(path, limits)?;
    let project = if data.starts_with(b"Extended Module:") { extract_xm(&data)? } else { extract_mod(&data)? };
    build_snapshot(&project, path)
}
