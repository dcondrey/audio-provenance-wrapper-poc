//! Parity with `daemon/audio_association.py`, the behavioural oracle.
//!
//! Every expected value in `fixtures/association_oracle.json` is produced by
//! `generate_parity_fixtures.py` calling the Python implementation. The WAV
//! inputs are built here from the same integer-only generator the fixture used
//! and checked by SHA-256, so both implementations read identical bytes.
//!
//! The tolerance is zero: every rendered field is a `round(x, n)` of the same
//! operations in the same order on the same doubles, so numbers are compared
//! bitwise and a divergence is a finding, not a reason to loosen a bound.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use apw_assoc::{
    associate_export, compare_feature_sequences, extract_feature_sequence, python_round,
    AssociationRecord, Feature, MAX_FEATURE_WINDOWS,
};
use apw_core::sha256_hex;
use serde_json::{json, Value};

const FIXTURE_TEXT: &str = include_str!("fixtures/association_oracle.json");

fn fixture() -> Value {
    serde_json::from_str(FIXTURE_TEXT).expect("fixture is valid JSON")
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate sits two levels below the repository root")
        .to_path_buf()
}

fn oracle_interpreter() -> Option<PathBuf> {
    let interpreter = repo_root().join(".venv/bin/python3.13");
    interpreter.is_file().then_some(interpreter)
}

// ------------------------------- comparison -------------------------------
fn compare(path: &str, actual: &Value, expected: &Value, failures: &mut Vec<String>) {
    match (actual, expected) {
        (Value::Object(left), Value::Object(right)) => {
            let left_keys: Vec<&String> = left.keys().collect();
            let right_keys: Vec<&String> = right.keys().collect();
            if left_keys != right_keys {
                failures.push(format!(
                    "{path}: key order differs\n  rust:   {left_keys:?}\n  python: {right_keys:?}"
                ));
                return;
            }
            for (key, expected_value) in right {
                compare(
                    &format!("{path}.{key}"),
                    left.get(key).unwrap_or(&Value::Null),
                    expected_value,
                    failures,
                );
            }
        }
        (Value::Array(left), Value::Array(right)) => {
            if left.len() != right.len() {
                failures.push(format!(
                    "{path}: length {} != {}",
                    left.len(),
                    right.len()
                ));
                return;
            }
            for (index, expected_value) in right.iter().enumerate() {
                compare(&format!("{path}[{index}]"), &left[index], expected_value, failures);
            }
        }
        (Value::Number(left), Value::Number(right)) => {
            if left.is_f64() != right.is_f64() {
                failures.push(format!("{path}: {left} and {right} are not the same JSON number kind"));
            } else if left.is_f64() {
                let (left_value, right_value) = (left.as_f64().unwrap(), right.as_f64().unwrap());
                if left_value.to_bits() != right_value.to_bits() {
                    failures.push(format!(
                        "{path}: {left_value:?} != {right_value:?} ({:#x} vs {:#x})",
                        left_value.to_bits(),
                        right_value.to_bits()
                    ));
                }
            } else if left != right {
                failures.push(format!("{path}: {left} != {right}"));
            }
        }
        _ => {
            if actual != expected {
                failures.push(format!("{path}: {actual} != {expected}"));
            }
        }
    }
}

fn assert_matches(label: &str, actual: &Value, expected: &Value) {
    let mut failures = Vec::new();
    compare(label, actual, expected, &mut failures);
    assert!(
        failures.is_empty(),
        "{} divergence(s) from the Python oracle:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ------------------------------ feature inputs ------------------------------
fn feature_from_json(value: &Value) -> Feature {
    Feature {
        rms: value.get("rms").and_then(Value::as_f64),
        zcr: value.get("zcr").and_then(Value::as_f64),
        crest: value.get("crest").and_then(Value::as_f64),
        envelope: value.get("envelope").and_then(Value::as_array).map(|values| {
            values
                .iter()
                .map(|band| band.as_f64().unwrap_or(f64::NAN))
                .collect()
        }),
    }
}

/// Mirrors `synthetic_feature` in the generator: every value is an integer
/// remainder divided by a power of two, so both languages hold identical bits.
fn synthetic_feature(seed: i64, index: i64, gain: f64, axes: &str) -> Feature {
    Feature {
        rms: Some((((seed * 7 + index * 13).rem_euclid(97) + 1) as f64 / 128.0) * gain),
        zcr: Some((seed * 5 + index * 29).rem_euclid(53) as f64 / 512.0),
        crest: axes
            .contains('c')
            .then(|| 1.0 + (seed + index).rem_euclid(7) as f64 / 8.0),
        envelope: axes.contains('e').then(|| {
            (0..4)
                .map(|band| (seed + index + band * 3).rem_euclid(5) as f64 / 4.0)
                .collect()
        }),
    }
}

fn materialise(side: &Value) -> Vec<Feature> {
    if let Some(Value::Array(explicit)) = side.get("explicit") {
        return explicit.iter().map(feature_from_json).collect();
    }
    let rule = side.get("synthetic").expect("side is explicit or synthetic");
    let seed = rule["seed"].as_i64().unwrap();
    let start = rule["start"].as_i64().unwrap();
    let count = rule["count"].as_i64().unwrap();
    let gain = rule["gain"].as_f64().unwrap();
    let axes = rule["axes"].as_str().unwrap();
    (0..count)
        .map(|index| synthetic_feature(seed, start + index, gain, axes))
        .collect()
}

// ------------------------------- WAV assembly -------------------------------
fn scaled(value: i64, numerator: i64, denominator: i64) -> i64 {
    let magnitude = value.abs() * numerator / denominator;
    if value < 0 {
        -magnitude
    } else {
        magnitude
    }
}

fn waveform(kind: i64, position: i64, period: i64, amplitude: i64) -> i64 {
    match kind {
        0 => {
            if position * 2 < period {
                amplitude
            } else {
                -amplitude
            }
        }
        1 => amplitude - (2 * amplitude * (2 * position - period).abs()) / period,
        2 => (2 * amplitude * position) / period - amplitude,
        _ => {
            if position == 0 {
                3 * amplitude
            } else {
                scaled(amplitude * ((position * 7).rem_euclid(5) - 2), 1, 8)
            }
        }
    }
}

fn analytic_samples(windows: i64, window_frames: i64) -> Vec<i64> {
    const PERIODS: [i64; 6] = [37, 5, 19, 3, 11, 71];
    const AMPLITUDES: [i64; 6] = [2600, 10100, 4600, 12100, 6200, 9200];
    const QUARTERS: [i64; 4] = [5, 10, 7, 9];
    let mut samples = Vec::new();
    for window in 0..windows {
        let period = PERIODS[(window % 6) as usize];
        let amplitude = AMPLITUDES[(window % 6) as usize];
        let kind = window % 4;
        for offset in 0..window_frames {
            let absolute = window * window_frames + offset;
            let quarter = QUARTERS[std::cmp::min(3, (offset * 4 / window_frames) as usize)];
            let value = scaled(waveform(kind, absolute % period, period, amplitude), quarter, 10);
            samples.push(value.clamp(-32768, 32767));
        }
    }
    samples
}

fn noise_samples(count: i64, seed: i64) -> Vec<i64> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state = (state * 1103515245 + 12345) % (1 << 31);
            state % 20001 - 10000
        })
        .collect()
}

fn spec_i64(spec: &Value, key: &str, default: i64) -> i64 {
    spec.get(key).and_then(Value::as_i64).unwrap_or(default)
}

fn build_samples(spec: &Value, window_frames: i64) -> Vec<i64> {
    if spec.get("samples").and_then(Value::as_str) == Some("noise") {
        return noise_samples(spec_i64(spec, "windows", 0) * window_frames, spec_i64(spec, "seed", 17));
    }
    let mut samples = analytic_samples(
        spec_i64(spec, "windows", 0),
        spec_i64(spec, "window_frames", window_frames),
    );
    let numerator = spec_i64(spec, "gain_numerator", 1);
    let denominator = spec_i64(spec, "gain_denominator", 1);
    if (numerator, denominator) != (1, 1) {
        samples = samples
            .iter()
            .map(|value| scaled(*value, numerator, denominator))
            .collect();
    }
    let lead = spec_i64(spec, "lead_silence_frames", 0);
    let mut leading = vec![0i64; lead as usize];
    leading.extend(samples);
    leading
}

fn frame_bytes(value: i64, sample_width: i64, float_format: bool, out: &mut Vec<u8>) {
    if float_format {
        out.extend_from_slice(&((value as f64 / 32768.0) as f32).to_le_bytes());
        return;
    }
    match sample_width {
        1 => out.push((scaled(value, 1, 256) + 128).clamp(0, 255) as u8),
        2 => out.extend_from_slice(&(value as i16).to_le_bytes()),
        3 => out.extend_from_slice(&(value * 256).to_le_bytes()[..3]),
        4 => out.extend_from_slice(&((value * 65536) as i32).to_le_bytes()),
        8 => out.extend_from_slice(&(value * 65536 * 65536).to_le_bytes()),
        other => panic!("unsupported test sample width {other}"),
    }
}

fn wav_bytes(spec: &Value, window_frames: i64) -> Vec<u8> {
    let samples = build_samples(spec, window_frames);
    let sample_width = spec_i64(spec, "sample_width", 2);
    let channels = spec_i64(spec, "channels", 1);
    let sample_rate = spec_i64(spec, "sample_rate", 44_100);
    let tag = spec_i64(spec, "format_tag", 1);
    let float_format = spec
        .get("float_format")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut data = Vec::new();
    for value in &samples {
        frame_bytes(*value, sample_width, float_format, &mut data);
        if channels == 2 {
            frame_bytes(scaled(*value, 6, 10), sample_width, float_format, &mut data);
        }
    }

    let bits = (sample_width * 8) as u16;
    let block_align = (channels * sample_width) as u16;
    let mut format = Vec::new();
    format.extend_from_slice(&(tag as u16).to_le_bytes());
    format.extend_from_slice(&(channels as u16).to_le_bytes());
    format.extend_from_slice(&(sample_rate as u32).to_le_bytes());
    format.extend_from_slice(&((sample_rate * i64::from(block_align)) as u32).to_le_bytes());
    format.extend_from_slice(&block_align.to_le_bytes());
    format.extend_from_slice(&bits.to_le_bytes());
    if tag == 0xFFFE {
        format.extend_from_slice(&22u16.to_le_bytes());
        format.extend_from_slice(&bits.to_le_bytes());
        format.extend_from_slice(&3u32.to_le_bytes());
        for byte in spec["subtype"].as_array().unwrap() {
            format.push(byte.as_u64().unwrap() as u8);
        }
    }

    let mut chunks = Vec::new();
    chunks.extend_from_slice(b"fmt ");
    chunks.extend_from_slice(&(format.len() as u32).to_le_bytes());
    chunks.extend_from_slice(&format);
    if spec.get("junk_chunk").and_then(Value::as_bool).unwrap_or(false) {
        let junk = b"junk payload!";
        chunks.extend_from_slice(b"JUNK");
        chunks.extend_from_slice(&(junk.len() as u32).to_le_bytes());
        chunks.extend_from_slice(junk);
        chunks.push(0);
    }
    if let Some(limit) = spec.get("truncate_fmt_to").and_then(Value::as_u64) {
        chunks.clear();
        chunks.extend_from_slice(b"fmt ");
        chunks.extend_from_slice(&(limit as u32).to_le_bytes());
        chunks.extend_from_slice(&format[..limit as usize]);
    }

    let mut data_chunk = Vec::new();
    data_chunk.extend_from_slice(b"data");
    data_chunk.extend_from_slice(&(data.len() as u32).to_le_bytes());
    data_chunk.extend_from_slice(&data);

    let mut body = Vec::new();
    if spec.get("data_before_fmt").and_then(Value::as_bool).unwrap_or(false) {
        body.extend_from_slice(&data_chunk);
        body.extend_from_slice(&chunks);
    } else {
        body.extend_from_slice(&chunks);
        body.extend_from_slice(&data_chunk);
    }

    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
    file.extend_from_slice(b"WAVE");
    file.extend_from_slice(&body);
    file
}

// ---------------------------------- tests ----------------------------------
#[test]
fn python_round_matches_cpython() {
    let fixture = fixture();
    let cases = fixture["round_cases"].as_array().unwrap();
    assert!(cases.len() >= 200, "round corpus shrank unexpectedly");
    let mut failures = Vec::new();
    for case in cases {
        let value = case[0].as_f64().unwrap();
        let digits = case[1].as_u64().unwrap() as usize;
        let expected = case[2].as_f64().unwrap();
        let produced = python_round(value, digits);
        if produced.to_bits() != expected.to_bits() {
            failures.push(format!("round({value:?}, {digits}) = {produced:?}, python {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn feature_comparison_matches_the_oracle() {
    let fixture = fixture();
    let cases = fixture["comparison_cases"].as_array().unwrap();
    assert!(cases.len() >= 15, "comparison corpus shrank unexpectedly");
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let routed = materialise(&case["routed"]);
        let exported = materialise(&case["exported"]);
        let window_seconds = case["window_seconds"].as_f64().unwrap();
        let record = compare_feature_sequences(&routed, &exported, window_seconds);
        assert_matches(name, &record.to_value(), &case["expected"]);
    }
}

#[test]
fn export_association_matches_the_oracle() {
    let fixture = fixture();
    let window_frames = fixture["window_frames"].as_i64().unwrap();
    let routed_events = fixture["routed_events"].as_array().unwrap();
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();

    for case in fixture["wav_cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let path = root.join(case["file"].as_str().unwrap());
        if case.get("missing").and_then(Value::as_bool) != Some(true) {
            let bytes = match case.get("raw").and_then(Value::as_str) {
                Some(raw) => raw.as_bytes().to_vec(),
                None => wav_bytes(&case["spec"], window_frames),
            };
            assert_eq!(
                sha256_hex(&bytes),
                case["sha256"].as_str().unwrap(),
                "{name}: the Rust generator did not reproduce the bytes the oracle read"
            );
            std::fs::write(&path, &bytes).expect("write the export");
        }
        let record = associate_export(&path, routed_events);
        let mut produced = record.to_value();
        if let Some(Value::String(reason)) = produced.get("reason") {
            let normalised = reason.replace(&root.to_string_lossy().to_string(), "<DIR>");
            produced["reason"] = json!(normalised);
        }
        assert_matches(name, &produced, &case["expected"]);
    }
}

#[test]
fn extraction_matches_the_oracle() {
    let fixture = fixture();
    let window_frames = fixture["window_frames"].as_i64().unwrap();
    let directory = tempfile::tempdir().expect("temporary directory");
    let root = directory.path();
    for case in fixture["wav_cases"].as_array().unwrap() {
        if case.get("missing").is_some() || case.get("raw").is_some() {
            continue;
        }
        std::fs::write(
            root.join(case["file"].as_str().unwrap()),
            wav_bytes(&case["spec"], window_frames),
        )
        .expect("write the export");
    }

    for case in fixture["extraction_cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let path = root.join(case["file"].as_str().unwrap());
        let target = case["target_window_seconds"].as_f64().unwrap();
        let max_windows = case["max_windows"].as_u64().unwrap() as usize;
        let (features, details) = extract_feature_sequence(&path, target, max_windows)
            .unwrap_or_else(|failure| panic!("{name}: extraction failed: {failure}"));
        let produced: Vec<Value> = features
            .iter()
            .map(|feature| {
                json!({
                    "rms": feature.rms.unwrap(),
                    "zcr": feature.zcr.unwrap(),
                    "crest": feature.crest.unwrap(),
                    "envelope": feature.envelope.clone().unwrap(),
                })
            })
            .collect();
        assert_matches(
            &format!("{name}.features"),
            &Value::Array(produced),
            &case["features"],
        );
        assert_matches(&format!("{name}.details"), &details.to_value(), &case["details"]);
    }
}

/// The record can never claim the export was observed being produced: the
/// proof level is derived from the status, and the comparison only ever
/// establishes an inference.
#[test]
fn no_reachable_record_claims_direct_observation() {
    let fixture = fixture();
    let mut seen = std::collections::BTreeSet::new();
    for case in fixture["comparison_cases"].as_array().unwrap() {
        let record = compare_feature_sequences(
            &materialise(&case["routed"]),
            &materialise(&case["exported"]),
            case["window_seconds"].as_f64().unwrap(),
        );
        seen.insert(record.proof_level().as_str().to_owned());
        assert_ne!(record.proof_level().as_str(), "directly_observed");
    }
    assert_eq!(
        AssociationRecord::unavailable("x").proof_level().as_str(),
        "unknown_unobserved"
    );
    assert!(seen.contains("inferred") && seen.contains("unknown_unobserved"));
    assert_eq!(MAX_FEATURE_WINDOWS, 12_000);
}

/// Fixture provenance: the committed expectations must still be what the
/// Python oracle produces from the same inputs today.
#[test]
fn the_committed_fixture_is_current() {
    let Some(interpreter) = oracle_interpreter() else {
        panic!(
            "the Python oracle is missing: create the repository venv at {}",
            repo_root().join(".venv").display()
        );
    };
    let directory = tempfile::tempdir().expect("temporary directory");
    let regenerated = directory.path().join("association_oracle.json");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/generate_parity_fixtures.py");
    let output = Command::new(&interpreter)
        .arg(&script)
        .arg(&regenerated)
        .output()
        .expect("run the fixture generator");
    assert!(
        output.status.success(),
        "generator failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut produced: Value =
        serde_json::from_str(&std::fs::read_to_string(&regenerated).expect("read regenerated"))
            .expect("regenerated fixture is valid JSON");
    let mut committed = fixture();
    let interpreter_path = produced["python"]["executable"].as_str().unwrap_or_default();
    assert!(
        interpreter_path.ends_with(".venv/bin/python3.13"),
        "the oracle ran under {interpreter_path}, not the repository venv"
    );
    produced.as_object_mut().unwrap().remove("python");
    committed.as_object_mut().unwrap().remove("python");
    assert_matches("fixture", &produced, &committed);
}

/// Two behaviours the fixture cannot reach, because the oracle's message for
/// them is an artefact of its environment rather than of the algorithm.
#[test]
fn the_seams_the_oracle_cannot_express() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let aiff = directory.path().join("export.aiff");
    std::fs::write(&aiff, b"FORM\0\0\0\0AIFF").expect("write the export");
    let record = associate_export(&aiff, fixture()["routed_events"].as_array().unwrap());
    assert_eq!(record.status.as_str(), "unavailable");
    assert!(
        record.reason.as_deref().unwrap_or_default().contains("AIFF"),
        "an AIFF export must say so, not fall through to the unsupported-suffix message"
    );

    // `_feature` divides by the length of each quarter segment, so a window
    // shorter than four samples is an error rather than a NaN envelope.
    assert!(Feature::from_samples(&[0.1, 0.2]).is_err());
    assert!(Feature::from_samples(&[0.1, 0.2, 0.3, 0.4]).is_ok());
    assert!(Feature::from_samples(&[]).is_ok());
}
