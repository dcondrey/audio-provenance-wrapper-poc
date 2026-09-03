#![allow(clippy::unwrap_used, clippy::panic)]

use audio_provenance_core::{CanonicalJsonError, CodedError, MAX_DEPTH, canonical_json};
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    let text = std::fs::read_to_string(format!("{path}{name}")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn cases(name: &str) -> Vec<Value> {
    load(name)
        .get("cases")
        .and_then(Value::as_array)
        .cloned()
        .unwrap()
}

fn field<'a>(case: &'a Value, key: &str) -> &'a str {
    case.get(key).and_then(Value::as_str).unwrap()
}

/// Every fixture is byte-identical to what
/// `/Volumes/A/audio-provenance/.venv/bin/python` produced from the apw-json-sort-v1
/// recipe. Regenerate with `tests/fixtures/generate.py`.
#[test]
fn matches_cpython_bytes_for_every_adversarial_case() {
    let cases = cases("canonical_cases.json");
    assert!(cases.len() > 20, "fixture set looks truncated");
    for case in &cases {
        let name = field(case, "name");
        let value: Value = serde_json::from_str(field(case, "input")).unwrap();
        let expected = hex::decode(field(case, "python_hex")).unwrap();
        let produced = canonical_json(&value).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&produced),
            String::from_utf8_lossy(&expected),
            "canonical bytes diverge from CPython for case {name}"
        );
        assert_eq!(produced, expected, "byte mismatch for case {name}");
    }
}

/// The float formatter is where CPython and every shortest-round-trip printer part company. The
/// fixture carries ~4000 random bit patterns plus the fixed/exponent threshold neighbours.
#[test]
fn float_formatting_matches_cpython_repr() {
    let cases = cases("float_cases.json");
    assert!(cases.len() > 3000, "float fixture set looks truncated");
    for case in &cases {
        let input = field(case, "input");
        let expected = field(case, "python");
        let value: Value = serde_json::from_str(input).unwrap();
        let produced = canonical_json(&value).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&produced),
            expected,
            "float formatting diverges from CPython for {input}"
        );
    }
}

/// CPython's canonicaliser has no depth bound; this crate rejects at 64 so a crafted manifest
/// cannot exhaust the verifier's stack. The fixtures record CPython's bytes only to show the
/// deviation is deliberate.
#[test]
fn rejects_nesting_beyond_the_documented_bound() {
    assert_eq!(MAX_DEPTH, 64);
    for case in &cases("depth_cases.json") {
        let name = field(case, "name");
        let value: Value = serde_json::from_str(field(case, "input")).unwrap();
        match canonical_json(&value) {
            Err(err @ CanonicalJsonError::DepthExceeded { .. }) => {
                assert_eq!(err.code(), "canonical_json_depth_exceeded");
            }
            other => panic!("case {name} should have exceeded the depth bound, got {other:?}"),
        }
        assert!(
            !field(case, "python_hex").is_empty(),
            "case {name} must record the CPython bytes it deliberately diverges from"
        );
    }
}

/// The depth bound counts every value, scalars included, exactly as `_validate_proof_values` in the
/// POC's `daemon/schema.py` does: the root sits at depth 0 and depth 64 is refused.
#[test]
fn depth_bound_counts_scalars_and_admits_exactly_sixty_four_levels() {
    let mut deepest_accepted = Value::Array(Vec::new());
    for _ in 0..63 {
        deepest_accepted = Value::Array(vec![deepest_accepted]);
    }
    assert!(canonical_json(&deepest_accepted).is_ok());

    let one_too_deep = Value::Array(vec![deepest_accepted.clone()]);
    assert!(matches!(
        canonical_json(&one_too_deep),
        Err(CanonicalJsonError::DepthExceeded { .. })
    ));

    let scalar_at_64 = {
        let mut value = Value::from(1u8);
        for _ in 0..64 {
            value = Value::Array(vec![value]);
        }
        value
    };
    assert!(matches!(
        canonical_json(&scalar_at_64),
        Err(CanonicalJsonError::DepthExceeded { .. })
    ));
}

/// `allow_nan=False` must be an error, not a silent `null` or a bare `NaN` token.
#[test]
fn non_finite_floats_are_a_typed_error() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let number = serde_json::Number::from_f64(value);
        assert!(
            number.is_none(),
            "serde_json unexpectedly admitted {value} into a Number"
        );
    }
    // serde_json cannot hold a non-finite in a Number, so the boundary this crate must own is the
    // one where a Number that reports a non-finite f64 reaches the writer.
    assert_eq!(
        CanonicalJsonError::NonFiniteFloat.code(),
        "canonical_json_non_finite_float"
    );
    assert!(serde_json::from_str::<Value>("NaN").is_err());
    assert!(serde_json::from_str::<Value>("Infinity").is_err());
    assert!(serde_json::from_str::<Value>("-Infinity").is_err());
}

/// FIXME: integers outside i64/u64 lose precision in `serde_json::Value` before canonicalisation
/// sees them, so their canonical bytes cannot match CPython's arbitrary-precision integers. Fixing
/// it needs `serde_json/arbitrary_precision`, which would in turn replace the float path with the
/// input's literal text and break the parity proven above. The divergence is asserted rather than
/// papered over.
#[test]
fn documented_big_integer_divergence_is_asserted() {
    for case in &cases("divergences.json") {
        let name = field(case, "name");
        let value: Value = serde_json::from_str(field(case, "input")).unwrap();
        let produced = String::from_utf8(canonical_json(&value).unwrap()).unwrap();
        assert_eq!(produced, field(case, "rust"), "divergence moved for {name}");
        assert_ne!(
            produced,
            field(case, "python"),
            "divergence {name} has closed; delete the fixture and fold it into the parity set"
        );
        assert!(!field(case, "reason").is_empty());
    }
}

/// The three `serde_json` feature assumptions canonicalisation rests on. `preserve_order` and
/// `arbitrary_precision` would break it if a sibling crate switched them on; `float_roundtrip` is
/// required and breaks it if anyone drops it from `[workspace.dependencies]`.
#[test]
fn serde_json_feature_assumptions_still_hold() {
    let value: Value = serde_json::from_str(r#"{"b":1,"a":2,"c":0.1}"#).unwrap();
    assert_eq!(canonical_json(&value).unwrap(), br#"{"a":2,"b":1,"c":0.1}"#);

    let float: Value = serde_json::from_str("1.0000").unwrap();
    assert_eq!(canonical_json(&float).unwrap(), b"1.0");

    let ulp: Value = serde_json::from_str("9.999999999999999e-05").unwrap();
    assert_eq!(
        ulp.as_f64().unwrap().to_bits(),
        0x3f1a_36e2_eb1c_432c,
        "serde_json parsed one ULP away from CPython; \
         `float_roundtrip` is missing from serde_json's features"
    );
}

/// Two defects this crate had to work around, each of which silently produced canonical bytes that
/// no Python-signed manifest could ever match. Both are pinned by value because both are one
/// dependency bump away from returning.
///
/// - `serde_json`'s default float parser is approximate: without `float_roundtrip` it decodes
///   `9.999999999999999e-05` one ULP away from the value CPython and `str::parse` agree on.
/// - Rust's shortest-form float printer breaks an exact decimal tie away from zero;
///   CPython's dtoa breaks it to even, so `-1084232472921069.25` prints as `...069.3` there and
///   `...069.2` here.
#[test]
fn float_parse_and_tie_break_regressions_stay_fixed() {
    for (input, expected) in [
        ("9.999999999999999e-05", "9.999999999999999e-05"),
        ("-1084232472921069.2", "-1084232472921069.2"),
        ("-1084232472921069.3", "-1084232472921069.2"),
        ("0.00001", "1e-05"),
        ("1e-7", "1e-07"),
        ("1e300", "1e+300"),
        ("1000000000000000.0", "1000000000000000.0"),
        ("1e16", "1e+16"),
        ("-0.0", "-0.0"),
    ] {
        let value: Value = serde_json::from_str(input).unwrap();
        let produced = String::from_utf8(canonical_json(&value).unwrap()).unwrap();
        assert_eq!(produced, expected, "regressed on {input}");
    }
}

/// Two documents Python distinguishes collapse onto one f64 in serde_json. Because the canonical
/// bytes are the signed content, accepting both would let a signature over one verify the other.
#[test]
fn integer_literals_wider_than_64_bits_are_refused_before_they_can_collapse() {
    let wide = br#"{"n":18446744073709551616}"#;
    let float_twin = br#"{"n":1.8446744073709552e19}"#;

    let error = audio_provenance_core::parse_signing_input(wide).unwrap_err();
    assert_eq!(
        audio_provenance_core::CodedError::code(&error),
        "canonical_json_integer_out_of_range"
    );

    let lax_wide: serde_json::Value = serde_json::from_slice(wide).unwrap();
    let lax_twin: serde_json::Value = serde_json::from_slice(float_twin).unwrap();
    assert_eq!(
        audio_provenance_core::canonical_json(&lax_wide).unwrap(),
        audio_provenance_core::canonical_json(&lax_twin).unwrap(),
        "the collapse this guard exists to prevent must still be real, or the guard is dead code"
    );

    let inside_range = br#"{"n":9223372036854775807,"m":-9223372036854775808}"#;
    assert!(audio_provenance_core::parse_signing_input(inside_range).is_ok());
}

#[test]
fn oversized_integers_inside_string_values_are_not_mistaken_for_numbers() {
    let payload = br#"{"note":"balance 18446744073709551616 exceeded","n":1}"#;
    assert!(audio_provenance_core::parse_signing_input(payload).is_ok());
}
