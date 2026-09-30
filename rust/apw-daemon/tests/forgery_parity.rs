//! `forgery_analysis`, byte for byte against the Python daemon. The event streams
//! and expected records live in `tests/fixtures/parity/forgery_analysis.json`,
//! produced by `derive_forgery_analysis` itself; no expected byte was written by
//! hand.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::collections::BTreeSet;

use apw_core::canonical_json_utf8;
use apw_daemon::{derive_forgery_analysis, ForgeryAnalyzer, StatisticalForgeryScreen};
use serde_json::Value;

mod common;

fn cases() -> Vec<Value> {
    common::parity_json("forgery_analysis.json")
}

#[test]
fn every_case_is_byte_identical_to_the_oracle() {
    let cases = cases();
    assert!(cases.len() >= 25, "the fixture lost its coverage");
    let mut flags = BTreeSet::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let events = case["events"].as_array().unwrap();
        let produced = derive_forgery_analysis(events);
        assert_eq!(
            String::from_utf8(canonical_json_utf8(&produced).unwrap()).unwrap(),
            String::from_utf8(canonical_json_utf8(&case["expected"]).unwrap()).unwrap(),
            "{name}"
        );
        // Insertion order is what the pretty manifest prints.
        assert_eq!(
            serde_json::to_string(&produced).unwrap(),
            serde_json::to_string(&case["expected"]).unwrap(),
            "{name}: key order or number rendering"
        );
        assert_eq!(StatisticalForgeryScreen.analyze(events), produced, "{name}");
        for analyzer in case["expected"]["analyzers"].as_object().unwrap().values() {
            for flag in analyzer["flags"].as_array().unwrap() {
                flags.insert(flag["name"].as_str().unwrap().to_owned());
            }
        }
    }
    assert_eq!(flags.len(), 12, "every flag must be exercised: {flags:?}");
}
