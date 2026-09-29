//! Host identification against the Python oracle (`tests/fixtures/parity/host_identity.json`),
//! and proof that the embedded table is the repository's `data/host_executables.json`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use apw_daemon::{identify_host, identify_host_in, parse_host_table, HostIdentity, MAX_HOST_TABLE_BYTES};
use serde_json::Value;

fn fixture() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/parity/host_identity.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn as_json(identity: &HostIdentity) -> Value {
    serde_json::json!({
        "recognised": identity.recognised,
        "host_name": identity.host_name,
        "identification": identity.identification,
        "proof_level": identity.proof_level,
        "host_id": identity.host_id,
        "display_name": identity.display_name,
        "source_url": identity.source_url,
    })
}

#[test]
fn the_embedded_table_is_byte_identical_to_the_repository_table() {
    let repository = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/host_executables.json")).unwrap();
    let embedded = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data/host_executables.json")).unwrap();
    assert_eq!(repository, embedded, "copy data/host_executables.json into rust/apw-daemon/data/");
}

#[test]
fn table_validation_matches_the_oracle() {
    let fixture = fixture();
    let cases = fixture["parse"].as_array().unwrap();
    assert!(cases.len() >= 35);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let raw: Vec<u8> = if let Some(hex) = case["raw_hex"].as_str() {
            apw_core::python_from_hex(hex).unwrap()
        } else if let Some(text) = case["raw"].as_str() {
            text.as_bytes().to_vec()
        } else {
            // A padded table of exactly `size` bytes.
            let size = usize::try_from(case["size"].as_u64().unwrap()).unwrap();
            let base = br#"{"schema_version": 1, "hosts": [{"host_id": "a", "display_name": "A", "source_url": "https://e.example/a", "match": [{"platform": "linux", "executable_name": "x"}]}]}"#;
            let mut data = base[..base.len() - 1].to_vec();
            data.resize(size - 1, b' ');
            data.push(b'}');
            data
        };
        let parsed = parse_host_table(&raw);
        match (case.get("ok"), case.get("error")) {
            (Some(ok), None) => {
                let table = parsed.unwrap_or_else(|error| panic!("{name}: rejected: {error}"));
                if ok.is_array() {
                    let mut rows: Vec<Value> = table
                        .entries()
                        .map(|(platform, exe, record)| {
                            serde_json::json!([platform, exe, record.host_id, record.display_name, record.source_url])
                        })
                        .collect();
                    rows.sort_by_key(ToString::to_string);
                    let mut expected = ok.as_array().unwrap().clone();
                    expected.sort_by_key(ToString::to_string);
                    assert_eq!(rows, expected, "{name}");
                }
            }
            (None, Some(error)) => {
                let message = parsed.err().unwrap_or_else(|| panic!("{name}: accepted a table Python rejects")).0;
                if case["message_exact"].as_bool() == Some(true) {
                    assert_eq!(Some(message.as_str()), error.as_str(), "{name}");
                }
            }
            _ => panic!("{name}: malformed fixture"),
        }
    }
    assert_eq!(MAX_HOST_TABLE_BYTES, 262_144);
}

#[test]
fn identification_matches_the_oracle_for_every_name_and_platform() {
    let fixture = fixture();
    let cases = fixture["identify"].as_array().unwrap();
    assert!(cases.len() > 250);
    let mut inferred = 0;
    for case in cases {
        let executable = case["executable_name"].as_str();
        let juce_name = case["juce_name"].as_str();
        let platform = case["platform"].as_str();
        let produced = identify_host(executable, case["juce_recognised"].as_bool().unwrap(), juce_name, platform).unwrap();
        assert_eq!(as_json(&produced), case["expected"], "{case}");
        if produced.identification == "inferred_from_executable_name" {
            assert_eq!(produced.proof_level, "inferred", "a table hit is never directly observed");
            inferred += 1;
        }
    }
    assert!(inferred > 40, "the table names must actually match");
}

#[test]
fn ambiguous_and_platform_specific_names_match_the_oracle() {
    let fixture = fixture();
    let table = parse_host_table(fixture["custom_table"].as_str().unwrap().as_bytes()).unwrap();
    for case in fixture["custom"].as_array().unwrap() {
        let produced = identify_host_in(
            &table,
            case["executable_name"].as_str(),
            false,
            None,
            case["platform"].as_str(),
        );
        assert_eq!(as_json(&produced), case["expected"], "{case}");
    }
}
