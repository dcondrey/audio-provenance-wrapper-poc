//! VCV Rack patches (`.vcv`): a port of `daemon/project_formats/vcv.py`; the
//! grounding (Rack v2.6.6) and the mapping onto the snapshot are documented there.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Map, Value};

use super::safe::{parse_json_capped, read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, digest_of, NeutralProject, NeutralTrack, ProjectSnapshot};
use super::tarzst::{inflate_zstd, read_tar, ZSTD_MAGIC};

const MAX_INT_DIGITS: usize = 4300;

fn int_or_none(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(number)) => number.as_i64(),
        _ => None,
    }
}

fn ident(value: Option<&Value>) -> String {
    int_or_none(value).map_or_else(|| "?".to_owned(), |number| number.to_string())
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

fn check_numbers(value: &Value) -> Result<()> {
    let mut stack = vec![value];
    while let Some(item) = stack.pop() {
        match item {
            Value::Number(number) => {
                let literal = number.to_string();
                if literal.contains(['.', 'e', 'E']) {
                    if !number.as_f64().is_some_and(f64::is_finite) {
                        return Err("malformed JSON: non-finite number".to_owned());
                    }
                } else if literal.trim_start_matches('-').len() > MAX_INT_DIGITS {
                    return Err(format!("malformed JSON: integer has more than {MAX_INT_DIGITS} digits"));
                }
            }
            Value::Object(map) => stack.extend(map.values()),
            Value::Array(items) => stack.extend(items.iter()),
            _ => {}
        }
    }
    Ok(())
}

fn objects(value: Option<&Value>) -> Vec<&Map<String, Value>> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_object).collect(),
        _ => Vec::new(),
    }
}

pub fn extract_vcv(data: &[u8], limits: &Limits) -> Result<NeutralProject> {
    let mut container = "json (legacy)";
    let mut raw: &[u8] = data;
    let tar;
    if data.get(..4) == Some(ZSTD_MAGIC.as_slice()) {
        container = "tar+zstd";
        tar = inflate_zstd(data, limits.max_tar_bytes)?;
        let members = read_tar(&tar, limits)?;
        let Some((_, start, size)) = members.iter().find(|(name, _, _)| name == "patch.json") else {
            return Err("VCV Rack archive has no patch.json".to_owned());
        };
        raw = tar.get(*start..*start + *size).unwrap_or_default();
    }
    // The archive is already capped at `max_tar_bytes`; a plain file at the project size cap.
    let document = parse_json_capped(raw, limits.max_tar_bytes as u64, limits)?;
    check_numbers(&document)?;
    let Some(version) = document.get("version").and_then(Value::as_str) else {
        return Err("not a VCV Rack patch: no top-level 'version' string".to_owned());
    };
    let root = document.as_object();
    let module_objects = objects(document.get("modules"));
    let cables = match root {
        Some(map) if map.contains_key("cables") => map.get("cables"),
        _ => document.get("wires"),
    };
    let cable_objects = objects(cables);

    let mut patch = NeutralTrack::new("patch".to_owned(), "patch".to_owned());
    patch.track_type = "Patch".to_owned();
    patch.devices = vec![
        format!("format: VCV Rack patch ({container})"),
        format!("rack version: {version}"),
        format!("modules: {}", module_objects.len()),
        format!("cables: {}", cable_objects.len()),
    ];
    let mut tracks = vec![patch];

    for (position, module) in module_objects.iter().enumerate() {
        let (plugin, model, module_version) = (text(module.get("plugin")), text(module.get("model")), text(module.get("version")));
        let params = module.get("params");
        let bypass = if module.contains_key("bypass") { module.get("bypass") } else { module.get("disabled") };
        let bypassed = matches!(bypass, Some(Value::Bool(true)));
        let data_value = module.get("data").filter(|value| !value.is_null());
        let mut devices = vec![format!("{plugin}/{model}")];
        if !module_version.is_empty() {
            devices.push(format!("version: {module_version}"));
        }
        let param_count = match params {
            Some(Value::Array(items)) => items.len(),
            _ => 0,
        };
        devices.push(format!("params: {param_count}"));
        if data_value.is_some() {
            devices.push("data: present".to_owned());
        }
        if bypassed {
            devices.push("bypassed".to_owned());
        }
        let track_id = match int_or_none(module.get("id")) {
            Some(id) => format!("module-{id}"),
            None => format!("module-pos-{position}"),
        };
        let mut track = NeutralTrack::new(track_id, format!("{plugin}/{model}"));
        track.track_type = "Module".to_owned();
        track.devices = devices;
        track.device_chain_hashes = BTreeSet::from([digest_of(&[json!({
            "plugin": plugin,
            "model": model,
            "version": module_version,
            "params": params.cloned().unwrap_or(Value::Null),
            "data": data_value.cloned().unwrap_or(Value::Null),
            "bypass": bypassed,
        })])]);
        tracks.push(track);
    }

    let endpoints: Vec<Value> = cable_objects
        .iter()
        .map(|cable| {
            Value::Array(
                ["outputModuleId", "outputId", "inputModuleId", "inputId"]
                    .iter()
                    .map(|key| int_or_none(cable.get(*key)).map_or(Value::Null, |number| json!(number)))
                    .collect(),
            )
        })
        .collect();
    let mut cable_track = NeutralTrack::new("cables".to_owned(), "cables".to_owned());
    cable_track.track_type = "Cables".to_owned();
    cable_track.devices = cable_objects
        .iter()
        .map(|cable| {
            format!(
                "{}:{} -> {}:{}",
                ident(cable.get("outputModuleId")),
                ident(cable.get("outputId")),
                ident(cable.get("inputModuleId")),
                ident(cable.get("inputId")),
            )
        })
        .collect();
    cable_track.device_chain_hashes = BTreeSet::from([digest_of(&[Value::Array(endpoints)])]);
    tracks.push(cable_track);
    Ok(NeutralProject::new("vcv_rack", tracks))
}

pub fn extract_vcv_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let data = read_project_bytes(path, limits)?;
    build_snapshot(&extract_vcv(&data, limits)?, path)
}
