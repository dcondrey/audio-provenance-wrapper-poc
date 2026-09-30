//! Every parser against the Python oracle's golden snapshots
//! (`tests/fixtures/projects/**/*.golden.json`), compared byte for byte on the
//! canonical JSON, plus the registry, the differ and `session_facts`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use apw_core::canonical_json_utf8;
use apw_daemon::project::{detect_format, snapshot_to_golden, Limits};
use serde_json::Value;

mod common;

fn projects_root() -> PathBuf {
    common::fixtures().join("projects")
}

fn golden_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            golden_files(&path, out);
        } else if path.to_string_lossy().ends_with(".golden.json") {
            out.push(path);
        }
    }
}

fn canonical(value: &Value) -> String {
    String::from_utf8(canonical_json_utf8(value).unwrap()).unwrap()
}

/// Fixture JSON can nest hundreds of levels (canonical forms of deep XML).
fn read_json(path: &Path) -> Value {
    let bytes = std::fs::read(path).unwrap();
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    deserializer.disable_recursion_limit();
    let value = <Value as serde::Deserialize>::deserialize(&mut deserializer).unwrap();
    std::mem::forget(deserializer);
    value
}

#[test]
fn every_golden_snapshot_is_reproduced() {
    let mut goldens = Vec::new();
    golden_files(&projects_root(), &mut goldens);
    goldens.sort();
    assert!(goldens.len() >= 5, "the golden fixtures went missing: {goldens:?}");
    for golden in goldens {
        let text = golden.to_string_lossy().into_owned();
        let source = PathBuf::from(text.strip_suffix(".golden.json").unwrap());
        let expected: Value = serde_json::from_slice(&std::fs::read(&golden).unwrap()).unwrap();
        let format = detect_format(&source).unwrap_or_else(|| panic!("{}: unregistered extension", source.display()));
        let snapshot = format
            .parse(&source, &Limits::default())
            .unwrap_or_else(|error| panic!("{}: {error}", source.display()));
        assert_eq!(
            canonical(&snapshot_to_golden(&snapshot)),
            canonical(&expected),
            "{}",
            source.display()
        );
    }
}

fn fixture() -> Value {
    read_json(&common::fixtures().join("parity/project_parity.json"))
}

fn all_sources() -> Vec<(String, PathBuf)> {
    let mut goldens = Vec::new();
    golden_files(&projects_root(), &mut goldens);
    goldens
        .into_iter()
        .map(|golden| {
            let text = golden.to_string_lossy().into_owned();
            let source = PathBuf::from(text.strip_suffix(".golden.json").unwrap());
            (source.strip_prefix(projects_root()).unwrap().to_string_lossy().into_owned(), source)
        })
        .collect()
}

#[test]
fn the_registry_matches_the_oracle() {
    use apw_daemon::project::FORMATS;
    let fixture = fixture();
    let expected = fixture["registry"].as_array().unwrap();
    assert_eq!(FORMATS.len(), expected.len());
    // Python registers the unsupported entries first; order is not semantic, since
    // lookup is by extension, so pair entries by id.
    for oracle in expected {
        let format = FORMATS.iter().find(|f| f.format_id == oracle["format_id"].as_str().unwrap()).unwrap();
        assert_eq!(format.host, oracle["host"].as_str().unwrap(), "{}", format.format_id);
        let extensions: Vec<&str> = oracle["extensions"].as_array().unwrap().iter().map(|e| e.as_str().unwrap()).collect();
        assert_eq!(format.extensions, extensions.as_slice(), "{}", format.format_id);
        assert_eq!(format.status.as_str(), oracle["status"].as_str().unwrap(), "{}", format.format_id);
        assert_eq!(format.reason, oracle["reason"].as_str().unwrap(), "{}", format.format_id);
        assert_eq!(format.validation, oracle["validation"].as_str().unwrap(), "{}", format.format_id);
    }
    // detect_format is by extension only, case-insensitively, and never reads the file.
    assert_eq!(detect_format(Path::new("/nonexistent/Song.LMMS")).map(|f| f.format_id), None);
    assert_eq!(detect_format(Path::new("/nonexistent/x.MMPZ")).map(|f| f.format_id), Some("lmms"));
    assert_eq!(detect_format(Path::new("/nonexistent/x")).map(|f| f.format_id), None);
}

#[test]
fn unsupported_events_match_the_oracle() {
    use apw_daemon::project::{unsupported_format_event, FORMATS};
    let fixture = fixture();
    for format in FORMATS.iter().filter(|f| !f.supported()) {
        let path = PathBuf::from(format!("/some/dir/Project{}", format.extensions[0].to_uppercase()));
        let mut event = unsupported_format_event(format, &path, 1, 2);
        let object = event.as_object_mut().unwrap();
        object.shift_remove("timestamp_ms");
        object.shift_remove("daemon_observed_monotonic_ms");
        assert_eq!(canonical(&event), canonical(&fixture["unsupported_events"][format.format_id]), "{}", format.format_id);
        assert!(format.parse(&path, &Limits::default()).unwrap_err().contains("are not supported"));
    }
}

#[test]
fn session_facts_and_diffs_match_the_oracle() {
    use apw_daemon::project::{compute_diff, diff_to_event, session_facts};
    let fixture = fixture();
    let mut snapshots = std::collections::BTreeMap::new();
    for (key, source) in all_sources() {
        let snapshot = detect_format(&source).unwrap().parse(&source, &Limits::default()).unwrap();
        assert_eq!(canonical(&session_facts(&snapshot)), canonical(&fixture["session_facts"][key.as_str()]), "{key}");
        snapshots.insert(key, snapshot);
    }
    for diff in fixture["diffs"].as_array().unwrap() {
        let (from, to) = (diff["from"].as_str().unwrap(), diff["to"].as_str().unwrap());
        let computed = compute_diff(&snapshots[from], &snapshots[to], 0);
        assert_eq!(computed.has_changes(), diff["has_changes"].as_bool().unwrap(), "{from} -> {to}");
        let mut event = diff_to_event(&computed, 0);
        for key in ["source_timestamp_ms", "timestamp_ms", "daemon_observed_monotonic_ms"] {
            event.shift_remove(key);
        }
        assert_eq!(canonical(&Value::Object(event.clone())), canonical(&diff["event"]), "{from} -> {to}");
        // Key order is what the evidence file records.
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            serde_json::to_string(diff["event"].as_object().unwrap()).unwrap(),
            "{from} -> {to}: key order"
        );
    }
}

#[test]
fn the_xml_reader_agrees_with_expat_and_elementtree() {
    use apw_daemon::project::snapshot::canonical_element;
    use apw_daemon::project::xml::{parse_xml, XmlLimits};
    let limits = XmlLimits { max_bytes: 256 * 1024 * 1024, max_depth: 128, max_elements: 2_000_000, et_mode: false };
    let fixture = fixture();
    let cases = fixture["xml"].as_array().unwrap();
    assert!(cases.len() >= 80);
    let (mut accepted, mut rejected) = (0, 0);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let data = apw_core::python_from_hex(case["hex"].as_str().unwrap()).unwrap();
        let doctype = case["allowed_doctype"].as_str();
        let produced = parse_xml(&data, doctype, limits);
        match (case["safe"].get("ok"), produced) {
            (Some(expected), Ok(root)) => {
                accepted += 1;
                assert_eq!(
                    serde_json::to_string(&canonical_element(&root, &[])).unwrap(),
                    serde_json::to_string(expected).unwrap(),
                    "{name}"
                );
            }
            (None, Err(_)) => rejected += 1,
            (Some(_), Err(error)) => panic!("{name}: rejected what expat accepts: {error}"),
            (None, Ok(_)) => panic!("{name}: accepted what expat rejects"),
        }
        if let Some(et) = case.get("et") {
            let produced = parse_xml(&data, None, XmlLimits { max_depth: 1024, et_mode: true, ..limits });
            match (et.get("ok"), produced) {
                (Some(expected), Ok(root)) => assert_eq!(root.to_xml_string(), expected.as_str().unwrap(), "{name}: ET.tostring"),
                (None, Err(_)) => {}
                (Some(_), Err(error)) => panic!("{name}: rejected what ElementTree accepts: {error}"),
                (None, Ok(_)) => panic!("{name}: accepted what ElementTree rejects"),
            }
        }
    }
    assert!(accepted >= 30 && rejected >= 40, "{accepted} accepted, {rejected} rejected");
}
