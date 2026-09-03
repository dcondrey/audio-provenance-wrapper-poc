//! Invariants that no fixture can pin because they are about rejection,
//! precedence and bounded growth rather than about bytes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use apw_core::{
    append_jsonl, canonical_json_of, canonical_json_utf8, rotated_evidence_paths, Canonicalization,
    CoreError, LocalOutcome, VerificationReport,
};
use serde_json::json;

#[derive(serde::Serialize)]
struct Reading {
    rms: f64,
}

/// REQUIRED: Python's `allow_nan=False` raises. `serde_json::to_value` would
/// turn the NaN into `null` and launder it into a signed manifest, so
/// `canonical_json_of` must reject at the serializer, not after.
#[test]
fn canonical_json_of_rejects_non_finite_floats() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let result = canonical_json_of(&Reading { rms: value }, Canonicalization::Utf8);
        assert!(
            matches!(result, Err(CoreError::NonFiniteNumber { .. })),
            "{value} was not rejected"
        );
    }
    assert_eq!(
        canonical_json_of(&Reading { rms: 0.5 }, Canonicalization::Utf8).unwrap(),
        b"{\"rms\":0.5}".to_vec()
    );
}

#[test]
fn nesting_beyond_the_bound_is_an_error_not_a_stack_overflow() {
    let mut value = json!({});
    for _ in 0..200 {
        value = json!({ "a": value });
    }
    assert!(matches!(
        canonical_json_utf8(&value),
        Err(CoreError::NestingTooDeep { .. })
    ));
}

/// The third precedence clause is "any error OR the
/// `portable_signature_missing` code", not "any error".
#[test]
fn outcome_precedence_follows_the_python_verifier() {
    let mut report = VerificationReport::new();
    assert_eq!(report.outcome(), LocalOutcome::Verified);

    report.warn("portable_signature_missing", "no portable signature");
    assert!(report.passed());
    assert_eq!(report.outcome(), LocalOutcome::Untrusted);

    report.info("evidence_hash_mismatch", "recorded as info on purpose");
    assert_eq!(report.outcome(), LocalOutcome::Changed);

    report.error("not_found", "manifest absent");
    assert_eq!(report.outcome(), LocalOutcome::NotFound);
}

#[test]
fn evidence_append_rotates_and_lists_oldest_first() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("evidence.jsonl");
    let record = json!({"event_type": "buffer_hash", "seq": 1});
    let encoded = canonical_json_utf8(&record).unwrap().len() as u64 + 1;

    for _ in 0..8 {
        append_jsonl(&path, &record, encoded * 2, 3).expect("append succeeds");
    }

    let paths = rotated_evidence_paths(&path).expect("listing succeeds");
    let names: Vec<String> = paths
        .iter()
        .map(|entry| entry.file_name().unwrap_or_default().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "evidence.3.jsonl",
            "evidence.2.jsonl",
            "evidence.1.jsonl",
            "evidence.jsonl",
        ]
    );
    for entry in &paths {
        assert!(
            std::fs::metadata(entry).unwrap().len() <= encoded * 2,
            "{} exceeded the cap",
            entry.display()
        );
    }

    // An oversize record is dropped, never truncated into the file.
    let before = std::fs::read(&path).unwrap();
    append_jsonl(&path, &record, 4, 3).expect("oversize append is not an error");
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Expected strings produced by `daemon.common.utc_timestamp` and by
/// `time.strftime("%Y-%m-%dT%H:%M:%SZ", gmtime())` for the same instants. The
/// microsecond field is present only when non-zero, and then always 6 digits.
#[test]
fn timestamps_match_the_python_isoformat_rules() {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    let cases: [(i64, u32, &str, &str); 7] = [
        (0, 0, "1970-01-01T00:00:00Z", "1970-01-01T00:00:00Z"),
        (1_756_600_000, 0, "2025-08-31T00:26:40Z", "2025-08-31T00:26:40Z"),
        (
            1_756_600_000,
            500_000_000,
            "2025-08-31T00:26:40.500000Z",
            "2025-08-31T00:26:40Z",
        ),
        (
            1_756_600_000,
            1_000,
            "2025-08-31T00:26:40.000001Z",
            "2025-08-31T00:26:40Z",
        ),
        (
            1_756_600_000,
            999_999_000,
            "2025-08-31T00:26:40.999999Z",
            "2025-08-31T00:26:40Z",
        ),
        (-2, 500_000_000, "1969-12-31T23:59:58.500000Z", "1969-12-31T23:59:58Z"),
        (951_782_400, 0, "2000-02-29T00:00:00Z", "2000-02-29T00:00:00Z"),
    ];
    for (seconds, nanos, iso, whole) in cases {
        let at = if seconds >= 0 {
            UNIX_EPOCH + Duration::new(seconds as u64, nanos)
        } else {
            UNIX_EPOCH - Duration::new(seconds.unsigned_abs(), 0) + Duration::new(0, nanos)
        };
        assert_eq!(apw_core::utc_timestamp(Some(at)), iso, "{seconds}.{nanos}");
        assert_eq!(
            apw_core::utc_timestamp_seconds(Some(at)),
            whole,
            "{seconds}.{nanos}"
        );
    }
    let now = apw_core::utc_timestamp(Some(SystemTime::now()));
    assert!(now.ends_with('Z') && now.len() >= 20, "{now}");
}
