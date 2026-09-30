//! `validate_network_event` against the Python validator, case by case, from
//! `tests/fixtures/parity/network_validation.json`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use apw_daemon::validate_network_event;
use serde_json::Value;

mod common;

#[test]
fn every_case_matches_the_python_validator() {
    let cases: Vec<Value> = common::parity_json("network_validation.json");
    assert!(cases.len() >= 30);
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let event = case["event"].as_object().unwrap();
        let produced = validate_network_event(event);
        if case["ok"].as_bool().unwrap() {
            assert!(produced.is_ok(), "{name}: rejected: {produced:?}");
        } else {
            let expected = case["message"].as_str().unwrap().to_string();
            assert_eq!(produced.err(), Some(expected), "{name}");
        }
    }
}
