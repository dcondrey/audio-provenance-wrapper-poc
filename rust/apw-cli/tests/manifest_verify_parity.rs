//! `apw verify <manifest>` against the Python verifier: every case in
//! `tests/fixtures/parity/manifest_verify.json` is replayed in a fresh directory and its
//! outcome, unrun checks and findings (severity, code, message) compared with what
//! `daemon/verify.py` recorded. `tests/test_parity_fixtures.py` replays the same cases
//! through Python.
//!
//! A case that carries a signature is signed here, after `{dir}` is substituted, with the
//! fixed seeds in the fixture and the real signers; Ed25519 and HMAC-SHA256 are deterministic, so the bytes
//! signed are the bytes Python signed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::Path;

use apw_cli::{verify_manifest, ManifestVerifyOptions};
use apw_core::{canonical_json, sha256_hex, without_top_level_keys, Canonicalization, Ed25519Signer};
use apw_provenance::{HardwareProvider, SoftwareProvider};
use serde_json::{json, Value};

fn hex(text: &str) -> Vec<u8> {
    apw_core::python_from_hex(text).expect("fixture hex")
}

fn write(path: &Path, content: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn set(target: &mut Value, changes: Value) {
    for (key, value) in changes.as_object().unwrap() {
        target[key] = value.clone();
    }
}

/// `seal_manifest` in `tests/fixtures/parity/generate_parity_fixtures.py`.
fn seal(manifest: &Value, pin_seed: &[u8], seal_seed: &[u8], scratch: &Path) -> Value {
    let unsigned = without_top_level_keys(manifest, &["portable_signature", "manifest_signature"]);
    let mut sealed = unsigned.clone();
    match manifest.get("portable_signature") {
        Some(Value::Object(base)) => {
            let private = scratch.join("portable-private.key");
            let public = scratch.join("portable-public.key");
            write(&private, pin_seed);
            let signature = Ed25519Signer::load_or_create(&private, &public).unwrap().sign_manifest(&unsigned).unwrap();
            let mut portable = Value::Object(base.clone());
            let signed = serde_json::to_value(&signature).unwrap();
            for key in ["public_key_hex", "signer_id", "signature_hex", "signed_content_hash"] {
                portable[key] = signed[key].clone();
            }
            sealed["portable_signature"] = portable;
        }
        Some(other) => sealed["portable_signature"] = other.clone(),
        None => {}
    }
    let signed_bytes = canonical_json(&sealed, Canonicalization::AsciiEscaped).unwrap();
    let key_path = scratch.join("seal-seed.key");
    write(&key_path, seal_seed);
    let provider = SoftwareProvider::new(&key_path).unwrap();
    let mut seal = manifest["manifest_signature"].clone();
    set(
        &mut seal,
        json!({
            "device_id": provider.device_identity().unwrap().device_id,
            "signature_hex": apw_provenance::hex_lower(&provider.sign(&signed_bytes).unwrap()),
            "signed_content_hash": sha256_hex(&signed_bytes),
        }),
    );
    sealed["manifest_signature"] = seal;
    sealed
}

fn apply_edits(manifest: &mut Value, edits: &Value) {
    for edit in edits.as_array().unwrap() {
        let path = edit["path"].as_array().unwrap();
        let (last, parents) = path.split_last().unwrap();
        let mut node = &mut *manifest;
        for step in parents {
            node = match step {
                Value::String(key) => &mut node[key.as_str()],
                index => &mut node[index.as_u64().unwrap() as usize],
            };
        }
        if edit.get("delete").is_some() {
            node.as_object_mut().unwrap().shift_remove(last.as_str().unwrap());
        } else {
            node[last.as_str().unwrap()] = edit["value"].clone();
        }
    }
}

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/parity")
}

/// `resolve_base` in the generator: a fixture file holding a manifest, or edits and files over
/// another base.
fn resolve_base(bases: &Value, name: &str) -> (Value, Vec<(String, Value)>) {
    match &bases[name] {
        Value::String(file) => {
            let document: Value = serde_json::from_slice(&std::fs::read(fixture_dir().join(file)).unwrap()).unwrap();
            (document["manifest"].clone(), Vec::new())
        }
        spec => {
            let (mut manifest, mut files) = resolve_base(bases, spec["from"].as_str().unwrap());
            apply_edits(&mut manifest, &spec["edits"]);
            files.extend(spec["files"].as_object().unwrap().iter().map(|(name, content)| (name.clone(), content.clone())));
            (manifest, files)
        }
    }
}

fn replay(case: &Value, bases: &Value) -> Value {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let dir = root.to_str().unwrap();
    let (template, mut files) = match case["base"].as_str() {
        Some(name) => resolve_base(bases, name),
        None => (Value::Null, Vec::new()),
    };
    files.extend(case["files"].as_object().unwrap().iter().map(|(name, content)| (name.clone(), content.clone())));
    for (name, content) in &files {
        match content.as_str() {
            Some(text) => write(&root.join(name), text.as_bytes()),
            None => {
                let _ = std::fs::remove_file(root.join(name));
            }
        }
    }
    let manifest_path = root.join("manifest.json");
    let body = if template.is_null() {
        case["raw"].clone()
    } else {
        let mut edited = template;
        apply_edits(&mut edited, &case["before"]);
        edited
    };
    match &body {
        Value::Null => {}
        Value::String(text) => write(&manifest_path, text.as_bytes()),
        template => {
            let mut manifest: Value = serde_json::from_str(&template.to_string().replace("{dir}", dir)).unwrap();
            if let Some(seeds) = case["seal"].as_object() {
                let pin_seed = hex(seeds["pin_seed_hex"].as_str().unwrap());
                let seal_seed = hex(seeds["seal_seed_hex"].as_str().unwrap());
                manifest = seal(&manifest, &pin_seed, &seal_seed, &root.join("scratch"));
                apply_edits(&mut manifest, &case["after"]);
            }
            write(&manifest_path, manifest.to_string().as_bytes());
        }
    }
    let pin = root.join("pin.key");
    if let Some(text) = case["pin_hex"].as_str() {
        write(&pin, &hex(text));
    }
    let seal_key = root.join("seal.key");
    if let Some(text) = case["seal_key_hex"].as_str() {
        write(&seal_key, &hex(text));
    }
    let anchor = root.join("anchor.pem");
    if let Some(text) = case["trust_anchor_pem"].as_str() {
        write(&anchor, text.as_bytes());
    }
    let options = ManifestVerifyOptions {
        signing_key: (!case["public_only"].as_bool().unwrap()).then_some(seal_key),
        public_key: pin,
        export: case["export_override"].as_str().map(|path| path.replace("{dir}", dir).into()),
        trust_anchor: case["trust_anchor_pem"].is_string().then_some(anchor),
        c2pa_asset: case["c2pa_asset"].as_str().map(|path| path.replace("{dir}", dir).into()),
        ..ManifestVerifyOptions::default()
    };
    let report = verify_manifest(&manifest_path, &options);
    json!({
        "outcome": report.outcome().as_str(),
        "passed": report.passed(),
        "checks_not_run": report.unchecked().iter().map(|(check, _)| *check).collect::<Vec<_>>(),
        "findings": report.findings.iter().map(|finding| json!({
            "severity": finding.severity.as_str(),
            "code": finding.code,
            "message": finding.message.replace(dir, "{dir}"),
        })).collect::<Vec<_>>(),
    })
}

#[test]
fn every_case_replays_to_the_findings_python_recorded() {
    let fixture: Value = serde_json::from_slice(&std::fs::read(fixture_dir().join("manifest_verify.json")).unwrap()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    let mut failures = Vec::new();
    for case in cases {
        let mut actual = replay(case, &fixture["bases"]);
        let expected = case["expected"].clone();
        // A recorded null message is Python's own error text: only the code is compared.
        for (index, finding) in expected["findings"].as_array().unwrap().clone().iter().enumerate() {
            if finding["message"].is_null() {
                actual["findings"][index]["message"] = Value::Null;
            }
        }
        if actual != expected {
            failures.push(format!("{}\n  rust:   {actual}\n  python: {expected}", case["name"].as_str().unwrap()));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases differ:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
