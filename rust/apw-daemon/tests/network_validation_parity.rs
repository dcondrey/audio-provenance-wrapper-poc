//! `validate_network_event` against the Python validator, case by case, from
//! `tests/fixtures/parity/network_validation.json`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use apw_daemon::validate_network_event;
use serde_json::Value;

#[test]
fn every_case_matches_the_python_validator() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/parity/network_validation.json");
    let cases: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert!(cases.len() >= 30);
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let event = case["event"].as_object().unwrap();
        let produced = validate_network_event(event);
        if case["ok"].as_bool().unwrap() {
            assert!(produced.is_ok(), "{name}: rejected: {produced:?}");
        } else {
            // Python 3.11+ renders a str-Enum in an f-string as `ProofLevel.X`; the
            // Rust wording is the value, which is what earlier Pythons printed.
            let expected = case["message"]
                .as_str()
                .unwrap()
                .replace("ProofLevel.DIRECTLY_OBSERVED", "directly_observed");
            assert_eq!(produced.err(), Some(expected), "{name}");
        }
    }
}
