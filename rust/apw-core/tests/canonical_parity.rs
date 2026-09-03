//! Byte-for-byte parity with the Python canonicalizers.
//!
//! The expected bytes are produced by `tests/generate_fixtures.py`, which calls
//! `daemon.common.canonical_json_bytes` and the two `json.dumps` call sites the
//! daemon uses. A divergence here means every signature produced by one
//! implementation fails verification in the other, so this test pins the whole
//! contract rather than any single rule.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use apw_core::{
    canonical_json_ascii, canonical_json_utf8, pretty_json_bytes, python_repr_f64,
};
use serde_json::Value;

fn cases() -> Vec<Value> {
    let raw = include_str!("fixtures/canonical_cases.json");
    match serde_json::from_str(raw).expect("fixture file is valid JSON") {
        Value::Array(cases) => cases,
        other => panic!("fixture root must be an array, got {other}"),
    }
}

fn field<'a>(case: &'a Value, key: &str) -> &'a str {
    case.get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("fixture case is missing {key}"))
}

#[test]
fn canonical_bytes_match_the_python_oracle() {
    let cases = cases();
    assert!(cases.len() >= 10, "fixture corpus shrank unexpectedly");
    for case in &cases {
        let name = field(case, "name");
        let value = case.get("value").expect("fixture case is missing value");

        let produced = canonical_json_utf8(value).expect("utf8 canonicalization");
        assert_eq!(
            String::from_utf8_lossy(&produced),
            field(case, "canonical_utf8"),
            "apw-json-sort-v1 diverged on case {name}"
        );

        let produced = canonical_json_ascii(value).expect("ascii canonicalization");
        assert_eq!(
            String::from_utf8_lossy(&produced),
            field(case, "canonical_ascii"),
            "apw-json-sort-ascii-v1 diverged on case {name}"
        );

        let produced = pretty_json_bytes(value).expect("pretty rendering");
        assert_eq!(
            String::from_utf8_lossy(&produced),
            field(case, "pretty"),
            "indent=2 rendering diverged on case {name}"
        );
    }
}

/// The float rules the fixtures exercise indirectly, pinned directly so a
/// regression names the offending value instead of a whole document.
#[test]
fn python_float_repr_thresholds() {
    let expected = [
        (0.0, "0.0"),
        (-0.0, "-0.0"),
        (1.0, "1.0"),
        (100.0, "100.0"),
        (1e-4, "0.0001"),
        (1e-5, "1e-05"),
        (1e-7, "1e-07"),
        (3.0517578125e-5, "3.0517578125e-05"),
        (1e15, "1000000000000000.0"),
        (9999999999999998.0, "9999999999999998.0"),
        (1e16, "1e+16"),
        (2e16 + 8.0, "2.000000000000001e+16"),
        (1e21, "1e+21"),
        (5e-324, "5e-324"),
        (1.7976931348623157e308, "1.7976931348623157e+308"),
        (0.1, "0.1"),
        (-2.5, "-2.5"),
    ];
    for (value, want) in expected {
        assert_eq!(python_repr_f64(value).unwrap(), want, "repr({value})");
    }
    assert!(python_repr_f64(f64::NAN).is_err());
    assert!(python_repr_f64(f64::INFINITY).is_err());
}
