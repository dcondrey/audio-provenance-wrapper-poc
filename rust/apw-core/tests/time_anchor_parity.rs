//! Golden fixtures from `tests/fixtures/parity/generate_parity_fixtures.py`. Every
//! expected value is produced by the Python oracle; nothing here is hand-written.

use apw_core::{
    anchored_record, canonical_json_utf8, check_time_anchor, encode_timestamp_request,
    parse_timestamp_request, parse_timestamp_response, sha256_hex, unavailable_anchor_record,
    verify_portable_signature, verify_time_proof, without_top_level_keys, Manifest,
    PortableSignature, TimeProof, VerificationReport, MAX_TSA_RESPONSE_BYTES,
    PORTABLE_SIGNATURE_EXCLUDED_KEYS,
};
use serde_json::Value;

type FindingRow = (String, String, String);
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn fixture(name: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/parity")
        .join(name);
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, Box<dyn std::error::Error>> {
    Ok(value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("fixture field {key} is not a string"))?)
}

fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, Box<dyn std::error::Error>> {
    Ok(value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("fixture field {key} is not an array"))?)
}

fn body_of(vector: &Value) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut body = apw_core::python_from_hex(text(vector, "response_hex")?)
        .ok_or("fixture response_hex is not hex")?;
    if let Some(pad) = vector.get("pad_zero_to").and_then(Value::as_u64) {
        body.resize(usize::try_from(pad)?, 0);
    }
    Ok(body)
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn response_parsing_matches_the_oracle() -> TestResult {
    let fixture = fixture("time_anchor.json")?;
    let vectors = array(&fixture, "parse")?;
    assert!(vectors.len() > 50, "the fixture lost its coverage");
    for vector in vectors {
        let name = text(vector, "name")?;
        let body = body_of(vector)?;
        let produced = parse_timestamp_response(&body);
        match (vector.get("ok"), vector.get("error")) {
            (Some(ok), None) => {
                let parsed = produced.map_err(|error| format!("{name}: rejected: {error}"))?;
                assert_eq!(Some(parsed.granted), ok.get("granted").and_then(Value::as_bool), "{name}");
                assert_eq!(
                    Some(parsed.status),
                    ok.get("status").and_then(Value::as_u64).map(u128::from),
                    "{name}"
                );
                assert_eq!(
                    parsed.gentime_ms,
                    ok.get("gentime_ms").and_then(Value::as_i64),
                    "{name}"
                );
                assert_eq!(
                    parsed.imprint_hash_hex.as_deref(),
                    ok.get("imprint_hash_hex").and_then(Value::as_str),
                    "{name}"
                );
                // Python renders the nonce as `format(int, "x")`: no leading zeros.
                let nonce = parsed.nonce.as_deref().map(|bytes| {
                    let digits = hex_of(bytes);
                    let trimmed = digits.trim_start_matches('0');
                    if trimmed.is_empty() { "0".to_owned() } else { trimmed.to_owned() }
                });
                assert_eq!(nonce.as_deref(), ok.get("nonce_hex").and_then(Value::as_str), "{name}");
            }
            (None, Some(error)) => {
                let message = produced
                    .err()
                    .ok_or_else(|| format!("{name}: accepted a response Python rejects"))?;
                if vector.get("message_exact").and_then(Value::as_bool) == Some(true) {
                    assert_eq!(Some(message.0.as_str()), error.as_str(), "{name}");
                }
            }
            _ => return Err(format!("{name}: malformed fixture").into()),
        }
    }
    Ok(())
}

#[test]
fn request_encoding_matches_the_oracle_and_round_trips() -> TestResult {
    let fixture = fixture("time_anchor.json")?;
    for vector in array(&fixture, "request")? {
        let name = text(vector, "name")?;
        let nonce = apw_core::python_from_hex(text(vector, "nonce_hex")?).ok_or("nonce hex")?;
        let produced = encode_timestamp_request(text(vector, "data_hash")?, &nonce);
        match vector.get("request_hex").and_then(Value::as_str) {
            Some(expected) => {
                let der = produced.map_err(|error| format!("{name}: {error}"))?;
                assert_eq!(hex_of(&der), expected, "{name}");
                let (hash, echoed) = parse_timestamp_request(&der)?;
                let trimmed: Vec<u8> = nonce.iter().copied().skip_while(|b| *b == 0).collect();
                assert_eq!(echoed, trimmed, "{name}");
                assert_eq!(hash.len(), 64, "{name}");
            }
            None => assert!(produced.is_err(), "{name}: encoded an input Python rejects"),
        }
    }
    Ok(())
}

#[test]
fn anchor_records_match_the_oracle() -> TestResult {
    let fixture = fixture("time_anchor.json")?;
    for vector in array(&fixture, "record")? {
        let name = text(vector, "name")?;
        let hash = text(vector, "data_hash")?;
        let url = text(vector, "tsa_url")?;
        let nonce = apw_core::python_from_hex(text(vector, "nonce_hex")?).ok_or("nonce hex")?;
        let body = body_of(vector)?;
        let expected = vector.get("expected").ok_or("missing expected record")?;
        let produced = match TimeProof::from_response(url, hash, &nonce, &body) {
            Ok(proof) => anchored_record(hash, &proof),
            Err(error) => unavailable_anchor_record(hash, &error.0),
        };
        if vector.get("message_exact").and_then(Value::as_bool) == Some(false) {
            assert_eq!(produced.get("status"), expected.get("status"), "{name}");
            continue;
        }
        assert_eq!(
            String::from_utf8(canonical_json_utf8(&produced)?)?,
            String::from_utf8(canonical_json_utf8(expected)?)?,
            "{name}"
        );
        // Key order is part of the signed-then-pretty-printed manifest bytes.
        let keys = |value: &Value| value.as_object().map(|map| map.keys().cloned().collect::<Vec<_>>());
        assert_eq!(keys(&produced), keys(expected), "{name}: key order");
        if expected.get("status").and_then(Value::as_str) == Some("anchored") {
            assert_eq!(expected.get("apw:proof_level").and_then(Value::as_str), Some("inferred"));
            assert_eq!(expected.get("cms_signature_verified"), Some(&Value::Bool(false)));
        }
    }
    let oversized = array(&fixture, "record")?
        .iter()
        .find(|vector| vector.get("name").and_then(Value::as_str) == Some("oversized_body"))
        .ok_or("no oversized vector")?;
    assert_eq!(body_of(oversized)?.len(), MAX_TSA_RESPONSE_BYTES + 1);
    Ok(())
}

#[test]
fn provider_verify_matches_the_oracle() -> TestResult {
    let fixture = fixture("time_anchor.json")?;
    for vector in array(&fixture, "provider_verify")? {
        let name = text(vector, "name")?;
        let proof = TimeProof {
            source: "s".to_owned(),
            timestamp_ms: i128::from(vector.get("timestamp_ms").and_then(Value::as_i64).ok_or("ms")?),
            nonce_hex: text(vector, "nonce_hex")?.to_owned(),
            response_hex: text(vector, "response_hex")?.to_owned(),
        };
        assert_eq!(
            Some(verify_time_proof(&proof, text(vector, "data_hash")?)),
            vector.get("expected").and_then(Value::as_bool),
            "{name}"
        );
    }
    Ok(())
}

fn findings_of(report: &VerificationReport) -> Vec<FindingRow> {
    report
        .findings
        .iter()
        .map(|finding| {
            (
                finding.severity.as_str().to_owned(),
                finding.code.clone(),
                finding.message.clone(),
            )
        })
        .collect()
}

fn expected_findings(vector: &Value, key: &str) -> Result<Vec<FindingRow>, Box<dyn std::error::Error>> {
    array(vector, key)?
        .iter()
        .map(|finding| {
            Ok((
                text(finding, "severity")?.to_lowercase(),
                text(finding, "code")?.to_owned(),
                text(finding, "message")?.to_owned(),
            ))
        })
        .collect()
}

#[test]
fn verifier_findings_match_the_oracle() -> TestResult {
    let fixture = fixture("time_anchor.json")?;
    let vectors = array(&fixture, "verify")?;
    assert!(vectors.len() > 40);
    for vector in vectors {
        let name = text(vector, "name")?;
        let mut report = VerificationReport::new();
        check_time_anchor(vector.get("manifest").ok_or("manifest")?, &mut report);
        assert_eq!(findings_of(&report), expected_findings(vector, "expected_findings")?, "{name}");
    }
    Ok(())
}

// --- the real Python-signed manifest -------------------------------------------

fn resolve<'a>(root: &'a Value, reference: &str) -> Result<&'a Value, Box<dyn std::error::Error>> {
    let name = reference
        .strip_prefix("#/$defs/")
        .ok_or_else(|| format!("unsupported $ref {reference}"))?;
    Ok(root
        .pointer(&format!("/$defs/{name}"))
        .ok_or_else(|| format!("unknown $ref {reference}"))?)
}

/// The JSON Schema subset docs/manifest.schema.json uses. An unknown keyword is a
/// test failure, never a silent skip. tests/fixtures/parity/minischema.py is the
/// Python twin.
fn validate(
    instance: &Value,
    schema: &Value,
    root: &Value,
    path: &str,
    errors: &mut Vec<String>,
) -> TestResult {
    const SUPPORTED: [&str; 19] = [
        "$schema", "$id", "$defs", "$ref", "title", "description", "type", "required",
        "properties", "allOf", "enum", "const", "items", "pattern", "minLength", "minimum",
        "maximum", "not", "additionalProperties",
    ];
    let Some(schema) = schema.as_object() else {
        return Err(format!("{path}: schema is not an object").into());
    };
    if let Some(unknown) = schema.keys().find(|key| !SUPPORTED.contains(&key.as_str())) {
        return Err(format!("{path}: unsupported schema keyword {unknown}").into());
    }
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        validate(instance, resolve(root, reference)?, root, path, errors)?;
    }
    for sub in schema.get("allOf").and_then(Value::as_array).into_iter().flatten() {
        validate(instance, sub, root, path, errors)?;
    }
    if let Some(wanted) = schema.get("type") {
        let names: Vec<&str> = match wanted {
            Value::String(name) => vec![name.as_str()],
            Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
            _ => return Err(format!("{path}: bad type").into()),
        };
        let matches = names.iter().any(|name| match *name {
            "object" => instance.is_object(),
            "array" => instance.is_array(),
            "string" => instance.is_string(),
            "number" => instance.is_number(),
            "integer" => instance.is_i64() || instance.is_u64(),
            "boolean" => instance.is_boolean(),
            "null" => instance.is_null(),
            _ => false,
        });
        if !matches {
            errors.push(format!("{path}: expected type {names:?}"));
        }
    }
    if let Some(constant) = schema.get("const") {
        if instance != constant {
            errors.push(format!("{path}: expected const {constant}"));
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array) {
        if !options.contains(instance) {
            errors.push(format!("{path}: {instance} is not one of {options:?}"));
        }
    }
    if let Some(forbidden) = schema.get("not") {
        let mut inner = Vec::new();
        validate(instance, forbidden, root, path, &mut inner)?;
        if inner.is_empty() {
            errors.push(format!("{path}: matches a forbidden schema"));
        }
    }
    if let Some(text) = instance.as_str() {
        if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64) {
            if (text.chars().count() as u64) < minimum {
                errors.push(format!("{path}: shorter than {minimum}"));
            }
        }
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
            if !matches_hex_pattern(pattern, text)? {
                errors.push(format!("{path}: does not match {pattern}"));
            }
        }
    }
    if let Some(number) = instance.as_f64().filter(|_| instance.is_number()) {
        if schema.get("minimum").and_then(Value::as_f64).is_some_and(|minimum| number < minimum) {
            errors.push(format!("{path}: below the minimum"));
        }
        if schema.get("maximum").and_then(Value::as_f64).is_some_and(|maximum| number > maximum) {
            errors.push(format!("{path}: above the maximum"));
        }
    }
    if let Some(object) = instance.as_object() {
        for key in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
            if let Some(key) = key.as_str().filter(|key| !object.contains_key(*key)) {
                errors.push(format!("{path}/{key}: required"));
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, sub) in properties {
                if let Some(child) = object.get(key) {
                    validate(child, sub, root, &format!("{path}/{key}"), errors)?;
                }
            }
            if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                for key in object.keys().filter(|key| !properties.contains_key(*key)) {
                    errors.push(format!("{path}/{key}: not allowed"));
                }
            }
        }
    }
    if let (Some(items), Some(list)) = (schema.get("items"), instance.as_array()) {
        for (index, item) in list.iter().enumerate() {
            validate(item, items, root, &format!("{path}/{index}"), errors)?;
        }
    }
    Ok(())
}

/// Only `^[0-9a-f]{N}$` occurs in the schema. Anything else fails the test so a
/// new pattern cannot pass unchecked.
fn matches_hex_pattern(pattern: &str, value: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let length = pattern
        .strip_prefix("^[0-9a-f]{")
        .and_then(|rest| rest.strip_suffix("}$"))
        .and_then(|digits| digits.parse::<usize>().ok())
        .ok_or_else(|| format!("unsupported pattern {pattern}"))?;
    Ok(value.len() == length && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
}

fn schema_errors(manifest: &Value) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let schema: Value = serde_json::from_slice(&std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manifest.schema.json"),
    )?)?;
    let mut errors = Vec::new();
    validate(manifest, &schema, &schema, "", &mut errors)?;
    Ok(errors)
}

#[test]
fn the_python_manifest_satisfies_the_schema_and_a_forged_one_does_not() -> TestResult {
    let fixture = fixture("manifest_rehearsal.json")?;
    let manifest = fixture.get("manifest").ok_or("manifest")?;
    assert_eq!(schema_errors(manifest)?, Vec::<String>::new());
    assert!(manifest.get("forgery_analysis").is_some());

    let mut broken = manifest.clone();
    *broken.pointer_mut("/time_anchor/status").ok_or("status")? = Value::from("verified");
    *broken.pointer_mut("/forgery_analysis/suspicion_score").ok_or("score")? = Value::from(1.5);
    let errors = schema_errors(&broken)?;
    assert!(errors.iter().any(|error| error.starts_with("/time_anchor/status")), "{errors:?}");
    assert!(
        errors.iter().any(|error| error.starts_with("/forgery_analysis/suspicion_score")),
        "{errors:?}"
    );
    Ok(())
}

#[test]
fn signing_bytes_of_the_python_manifest_are_reproduced() -> TestResult {
    let fixture = fixture("manifest_rehearsal.json")?;
    let manifest = Manifest::from_value(fixture.get("manifest").ok_or("manifest")?.clone())?;
    let portable = manifest.portable_signing_input()?;
    let local = manifest.local_signing_input()?;
    assert_eq!(sha256_hex(&portable), text(&fixture, "portable_signing_input_sha256")?);
    assert_eq!(Some(portable.len() as u64), fixture.get("portable_signing_input_length").and_then(Value::as_u64));
    assert_eq!(sha256_hex(&local), text(&fixture, "local_signing_input_sha256")?);
    assert_eq!(Some(local.len() as u64), fixture.get("local_signing_input_length").and_then(Value::as_u64));

    // The Ed25519 signature Python made over those bytes must verify in Rust.
    let signature: PortableSignature = serde_json::from_value(
        manifest.get("portable_signature").ok_or("portable_signature")?.clone(),
    )?;
    let unsigned = without_top_level_keys(manifest.as_value(), &PORTABLE_SIGNATURE_EXCLUDED_KEYS);
    assert_eq!(
        verify_portable_signature(&unsigned, &signature, None),
        Ok(apw_core::PORTABLE_SIGNATURE_VALID_MESSAGE)
    );
    Ok(())
}

#[test]
fn the_python_manifest_time_anchor_verifies_and_tampering_is_caught() -> TestResult {
    let fixture = fixture("manifest_rehearsal.json")?;
    let manifest = fixture.get("manifest").ok_or("manifest")?;
    let mut report = VerificationReport::new();
    check_time_anchor(manifest, &mut report);
    assert_eq!(findings_of(&report), expected_findings(&fixture, "time_anchor_findings")?);

    let mut tampered = manifest.clone();
    let stamp = tampered.pointer_mut("/time_anchor/timestamp_ms").ok_or("timestamp_ms")?;
    *stamp = Value::from(stamp.as_i64().ok_or("integer timestamp")? + 1000);
    let mut report = VerificationReport::new();
    check_time_anchor(&tampered, &mut report);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings.first().map(|f| f.code.as_str()), Some("time_anchor_invalid"));
    Ok(())
}
