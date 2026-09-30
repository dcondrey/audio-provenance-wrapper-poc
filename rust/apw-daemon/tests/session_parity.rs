//! Session bookkeeping that lands in the signed manifest: the plug-in telemetry
//! table, its regression counter and the graded `host_environment` record. Cases
//! come from `tests/fixtures/parity/session_state.json`, produced by driving the
//! Python `Daemon._record_plugin_event` and `derive_host_environment`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use apw_core::canonical_json_utf8;
use apw_daemon::{derive_host_environment_for, event_type_to_layer, SessionState};
use serde_json::Value;

mod common;

#[test]
fn every_session_case_matches_the_oracle() {
    let cases: Vec<Value> = common::parity_json("session_state.json");
    assert!(cases.len() >= 15);
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let mut session = SessionState::new();
        for event in case["events"].as_array().unwrap() {
            let layer = event_type_to_layer(event["event_type"].as_str().unwrap());
            session.record_plugin_event(event, layer);
        }
        let telemetry: Vec<Value> = session
            .telemetry()
            .into_iter()
            .map(|(key, value)| serde_json::json!([key, value]))
            .collect();
        assert_eq!(Value::Array(telemetry), case["telemetry"], "{name}: telemetry table");
        assert_eq!(
            Some(session.telemetry_regressions()),
            case["telemetry_regressions"].as_u64(),
            "{name}: regressions"
        );
        let platform = case["platform"].as_str().unwrap();
        let produced = derive_host_environment_for(&session.snapshot(), Some(platform));
        assert_eq!(
            serde_json::to_string(&produced).unwrap(),
            serde_json::to_string(&case["host_environment"]).unwrap(),
            "{name}: host_environment (key order included)"
        );
        assert_eq!(
            canonical_json_utf8(&produced).unwrap(),
            canonical_json_utf8(&case["host_environment"]).unwrap(),
            "{name}"
        );
    }
}
