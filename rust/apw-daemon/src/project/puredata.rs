//! Pure Data patches (`.pd`): a port of `daemon/project_formats/puredata.py`.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{json, Value};

use super::safe::{read_project_bytes, Limits, Result};
use super::snapshot::{build_snapshot, digest_of, NeutralProject, NeutralTrack, ProjectSnapshot};

const AUDIO_EXTENSIONS: [&str; 8] = [".wav", ".aif", ".aiff", ".flac", ".ogg", ".mp3", ".w64", ".caf"];
const BOX_TYPES: [&str; 5] = ["obj", "msg", "floatatom", "symbolatom", "text"];

/// Split Pd text into statements of atoms.
pub fn tokenize(text: &str, limits: &Limits) -> Result<Vec<Vec<String>>> {
    let mut statements: Vec<Vec<String>> = Vec::new();
    let mut atoms: Vec<String> = Vec::new();
    let mut atom = String::new();
    let mut in_atom = false;
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let flush = |atoms: &mut Vec<String>, atom: &mut String, in_atom: &mut bool| {
        if *in_atom {
            atoms.push(std::mem::take(atom));
            *in_atom = false;
        }
    };
    while let Some(&c) = chars.get(index) {
        if let (true, Some(&escaped)) = (c == '\\', chars.get(index + 1)) {
            atom.push(escaped);
            in_atom = true;
            index += 2;
            continue;
        }
        if c == ';' {
            flush(&mut atoms, &mut atom, &mut in_atom);
            statements.push(std::mem::take(&mut atoms));
            if statements.len() > limits.max_pd_statements {
                return Err(format!(
                    "Pd patch has more than {} statements; refusing to parse",
                    limits.max_pd_statements
                ));
            }
        } else if c == ',' {
            flush(&mut atoms, &mut atom, &mut in_atom);
            atoms.push(",".to_owned());
        } else if super::safe::is_python_space(c) || c.is_whitespace() {
            flush(&mut atoms, &mut atom, &mut in_atom);
        } else {
            atom.push(c);
            in_atom = true;
        }
        index += 1;
    }
    flush(&mut atoms, &mut atom, &mut in_atom);
    if !atoms.is_empty() {
        statements.push(atoms);
    }
    Ok(statements.into_iter().filter(|statement| !statement.is_empty()).collect())
}

struct Canvas {
    canvas_id: String,
    name: String,
    kind: &'static str,
    group: String,
    devices: Vec<String>,
    samples: Vec<String>,
    normalised: Vec<Value>,
}

fn atom_list(items: &[String]) -> Value {
    Value::Array(items.iter().map(|item| json!(item)).collect())
}

pub fn extract_pd(text: &str, limits: &Limits) -> Result<NeutralProject> {
    let statements = tokenize(text, limits)?;
    let first_ok = statements
        .first()
        .is_some_and(|first| first.first().map(String::as_str) == Some("#N") && first.get(1).map(String::as_str) == Some("canvas"));
    if !first_ok {
        return Err("not a Pd patch: first record is not '#N canvas'".to_owned());
    }
    let mut canvases: Vec<Canvas> = Vec::new();
    // Indexes into `canvases`, innermost last.
    let mut stack: Vec<usize> = Vec::new();
    for record in &statements {
        let head = record.first().map(String::as_str).unwrap_or_default();
        let kind = record.get(1).map(String::as_str).unwrap_or_default();
        if head == "#N" && kind == "canvas" {
            if stack.len() >= limits.max_pd_canvas_depth {
                return Err(format!("Pd canvas nesting deeper than {}; refusing to parse", limits.max_pd_canvas_depth));
            }
            if canvases.len() >= limits.max_pd_canvases {
                return Err(format!("Pd patch has more than {} canvases; refusing to parse", limits.max_pd_canvases));
            }
            let canvas = if stack.is_empty() {
                if !canvases.is_empty() {
                    return Err("Pd patch has a second top-level canvas".to_owned());
                }
                Canvas {
                    canvas_id: "canvas-0".to_owned(),
                    name: "main".to_owned(),
                    kind: "Patch",
                    group: String::new(),
                    devices: Vec::new(),
                    samples: Vec::new(),
                    normalised: Vec::new(),
                }
            } else {
                let name = record.get(6).cloned().unwrap_or_default();
                let parent = stack.last().and_then(|index| canvases.get(*index)).map(|c| c.canvas_id.clone()).unwrap_or_default();
                Canvas {
                    canvas_id: format!("canvas-{}", canvases.len()),
                    kind: if name == "(subpatch)" { "Graph" } else { "Subpatch" },
                    name,
                    group: parent,
                    devices: Vec::new(),
                    samples: Vec::new(),
                    normalised: Vec::new(),
                }
            };
            canvases.push(canvas);
            stack.push(canvases.len() - 1);
            continue;
        }
        let Some(current) = stack.last().copied() else {
            return Err("Pd record outside the root canvas".to_owned());
        };
        if head == "#X" && kind == "restore" {
            if stack.len() == 1 {
                return Err("Pd 'restore' without an open subpatch".to_owned());
            }
            stack.pop();
            let mut entry = vec![json!("restore")];
            entry.extend(record.iter().skip(4).map(|atom| json!(atom)));
            if let Some(parent) = stack.last().and_then(|index| canvases.get_mut(*index)) {
                parent.normalised.push(Value::Array(entry));
            }
        } else if head == "#X" {
            let Some(canvas) = canvases.get_mut(current) else { continue };
            // Box coordinates (records 2 and 3) are layout, not content.
            if BOX_TYPES.contains(&kind) {
                let mut entry = vec![json!(kind)];
                entry.extend(record.iter().skip(4).map(|atom| json!(atom)));
                canvas.normalised.push(Value::Array(entry));
            } else {
                canvas.normalised.push(atom_list(record.get(1..).unwrap_or_default()));
            }
            if kind == "obj" {
                canvas.devices.push(record.iter().skip(4).cloned().collect::<Vec<_>>().join(" "));
            }
            if kind == "obj" || kind == "msg" {
                canvas.samples.extend(
                    record
                        .iter()
                        .skip(4)
                        .filter(|atom| {
                            let lower = atom.to_lowercase();
                            AUDIO_EXTENSIONS.iter().any(|extension| lower.ends_with(extension))
                        })
                        .cloned(),
                );
            }
        } else if let Some(canvas) = canvases.get_mut(current) {
            canvas.normalised.push(atom_list(record));
        }
    }
    let tracks = canvases
        .into_iter()
        .map(|canvas| {
            let mut track = NeutralTrack::new(canvas.canvas_id, canvas.name);
            track.track_type = canvas.kind.to_owned();
            track.devices = canvas.devices;
            track.device_chain_hashes = BTreeSet::from([digest_of(&[Value::Array(canvas.normalised)])]);
            let mut seen: Vec<String> = Vec::new();
            for sample in canvas.samples {
                if !seen.contains(&sample) {
                    seen.push(sample);
                }
            }
            track.extra_sample_refs = seen;
            track.group_id = canvas.group;
            track
        })
        .collect();
    Ok(NeutralProject::new("pure_data", tracks))
}

pub fn extract_pd_snapshot(path: &Path, limits: &Limits) -> Result<ProjectSnapshot> {
    let bytes = read_project_bytes(path, limits)?;
    let text = String::from_utf8_lossy(&bytes);
    build_snapshot(&extract_pd(&text, limits)?, path)
}
