//! The differential itself: run `c2patool`, run this crate, and require they agree.
//!
//! `#[ignore]`d because it shells out to a binary that is not on every machine. Run it with:
//!
//! ```text
//! cargo test -p audio-provenance-c2pa --test c2patool_differential -- --ignored --nocapture
//! ```
//!
//! Last run here against **c2patool 0.26.68** (c2pa-rs 0.89.0) on 2026-08-31. The literals in
//! `tests/conformance.rs` are this test's output frozen, so the default suite keeps checking the
//! same agreement on a machine with no C2PA tool installed.

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

use audio_provenance_c2pa::{
    C2paClaim, C2paEvidence, C2paValidationState, ClaimSignatureEvidence, CredentialEvidence,
    HardBindingEvidence, classify,
};
use audio_provenance_manifest::C2paStore;
use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn c2patool(args: &[&str]) -> Value {
    let output = Command::new("c2patool")
        .args(args)
        .output()
        .expect("c2patool must be on PATH to run the differential");
    assert!(
        output.status.success(),
        "c2patool {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("c2patool emits JSON")
}

fn base64_to_hex(encoded: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = 0u32;
    let mut held = 0u32;
    let mut bytes = Vec::new();
    for symbol in encoded.bytes().filter(|b| *b != b'=') {
        let value = ALPHABET
            .iter()
            .position(|c| *c == symbol)
            .expect("standard base64 alphabet") as u32;
        bits = (bits << 6) | value;
        held += 6;
        if held >= 8 {
            held -= 8;
            bytes.push(u8::try_from((bits >> held) & 0xFF).unwrap());
        }
    }
    hex::encode(bytes)
}

fn codes(report: &Value, bucket: &str) -> Vec<String> {
    report["validation_results"]["activeManifest"][bucket]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|r| r["code"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The tool's own result codes, read back as the evidence [`classify`] consumes. This is the whole
/// point of the mapping: c2patool never emits a state, only facts.
fn evidence_from(report: &Value) -> C2paEvidence {
    let success = codes(report, "success");
    let failure = codes(report, "failure");
    let has = |bucket: &[String], code: &str| bucket.iter().any(|c| c == code);

    C2paEvidence {
        store_present: report["active_manifest"].is_string(),
        assertion_store_intact: has(&success, "assertion.hashedURI.match")
            && !has(&failure, "assertion.hashedURI.mismatch")
            && !has(&failure, "assertion.missing"),
        signature: if has(&success, "claimSignature.validated") {
            ClaimSignatureEvidence::Validated
        } else {
            ClaimSignatureEvidence::Invalid
        },
        hard_binding: if has(&success, "assertion.dataHash.match") {
            HardBindingEvidence::Match
        } else if has(&failure, "assertion.dataHash.mismatch") {
            HardBindingEvidence::Mismatch
        } else {
            HardBindingEvidence::Absent
        },
        credential: if has(&failure, "signingCredential.untrusted") {
            CredentialEvidence::Untrusted
        } else {
            CredentialEvidence::Trusted
        },
    }
}

fn compare(report: &Value, store: &C2paStore, asset: &[u8]) {
    let manifest_id = report["active_manifest"].as_str().unwrap();
    let tool_claim = &report["manifests"][manifest_id]["claim"];

    let claim = C2paClaim::from_store(store).unwrap();
    assert_eq!(claim.manifest_label(), manifest_id, "manifest label");
    assert_eq!(
        claim.alg(),
        tool_claim["alg"].as_str().unwrap(),
        "claim alg"
    );
    assert_eq!(
        u64::from(claim.version().number()),
        tool_claim["claim_version"].as_u64().unwrap_or(2),
        "claim version"
    );
    assert_eq!(
        claim.title(),
        tool_claim["dc:title"].as_str(),
        "claim title"
    );
    assert_eq!(
        claim.instance_id(),
        tool_claim["instanceID"].as_str(),
        "claim instance id"
    );
    assert_eq!(
        claim.signature_ref(),
        tool_claim["signature"].as_str(),
        "signature reference"
    );

    let mut expected = Vec::new();
    for field in ["created_assertions", "gathered_assertions"] {
        for uri in tool_claim[field].as_array().into_iter().flatten() {
            expected.push((
                uri["url"].as_str().unwrap().to_owned(),
                base64_to_hex(uri["hash"].as_str().unwrap()),
            ));
        }
    }
    let ours: Vec<(String, String)> = claim
        .assertions()
        .iter()
        .map(|u| (u.url().to_owned(), u.hash_hex()))
        .collect();
    assert_eq!(ours, expected, "hashed URIs");

    // We recompute each digest from the store's bytes; c2patool independently reported one
    // `assertion.hashedURI.match` per assertion. Both must agree that the store is intact.
    let integrity = claim.verify_assertions(store).unwrap();
    assert!(integrity.intact(), "{:?}", integrity.checks());

    let binding = store.hard_binding().unwrap();
    let ours_binding = HardBindingEvidence::from_outcome(binding.evaluate(asset).unwrap());
    let tool = evidence_from(report);
    assert_eq!(ours_binding, tool.hard_binding, "hard binding verdict");
    assert!(
        codes(report, "success")
            .iter()
            .any(|c| c == "assertion.dataHash.match"),
        "c2patool validated the data hash"
    );

    // The full mapping, driven end to end by the tool's own findings.
    assert_eq!(
        classify(tool),
        C2paValidationState::MarkFoundClaimNotTrusted,
        "a self-issued signer that no anchor covers is untrusted however intact the bytes are"
    );
    assert_eq!(
        classify(tool).to_status(),
        audio_provenance_core::VerificationStatus::Untrusted
    );
}

#[test]
#[ignore = "needs c2patool on PATH"]
fn an_embedded_wav_reads_the_same_through_c2patool_and_through_us() {
    let path = fixture("poc_unobserved_export_c2pa.wav");
    let asset = std::fs::read(&path).unwrap();
    let report = c2patool(&["-d", path.to_str().unwrap()]);
    assert_eq!(report["validation_state"], "Valid");

    let store = C2paStore::from_wav(&asset).unwrap();
    compare(&report, &store, &asset);
}

#[test]
#[ignore = "needs c2patool on PATH"]
fn an_aiff_sidecar_reads_the_same_through_c2patool_and_through_us() {
    let asset_path = fixture("poc_sample.aiff");
    let sidecar_path = fixture("poc_sample_sidecar.c2pa");
    let asset = std::fs::read(&asset_path).unwrap();
    let sidecar = std::fs::read(&sidecar_path).unwrap();

    let report = c2patool(&[
        "-d",
        asset_path.to_str().unwrap(),
        "--external-manifest",
        sidecar_path.to_str().unwrap(),
    ]);
    assert_eq!(report["validation_state"], "Valid");

    let store = C2paStore::from_sidecar(&sidecar).unwrap();
    compare(&report, &store, &asset);
}

/// The tool and this crate must also agree that a changed asset is changed, not merely untrusted.
/// One flipped audio byte, and both the recompute here and c2patool's `assertion.dataHash.mismatch`
/// have to move together.
#[test]
#[ignore = "needs c2patool on PATH"]
fn a_flipped_audio_byte_reads_as_changed_in_both() {
    let path = fixture("poc_unobserved_export_c2pa.wav");
    let mut asset = std::fs::read(&path).unwrap();
    asset[50_000] ^= 0x01;

    let dir = std::env::temp_dir().join("audio-provenance-c2pa-differential");
    std::fs::create_dir_all(&dir).unwrap();
    let altered = dir.join("altered.wav");
    std::fs::write(&altered, &asset).unwrap();

    let report = c2patool(&["-d", altered.to_str().unwrap()]);
    let tool = evidence_from(&report);
    assert_eq!(tool.hard_binding, HardBindingEvidence::Mismatch);

    let store = C2paStore::from_wav(&asset).unwrap();
    let ours = store.hard_binding().unwrap().evaluate(&asset).unwrap();
    assert_eq!(HardBindingEvidence::from_outcome(ours), tool.hard_binding);

    assert_eq!(classify(tool), C2paValidationState::RegisteredButChanged);
    assert_eq!(
        classify(tool).to_status(),
        audio_provenance_core::VerificationStatus::Changed
    );

    std::fs::remove_file(&altered).unwrap();
}

/// The check every single-manifest fixture is blind to: with two manifests in the store, does our
/// reader name the same one c2patool calls `active_manifest`?
#[test]
#[ignore = "needs c2patool on PATH"]
fn the_active_manifest_of_a_two_manifest_store_agrees_with_c2patool() {
    let path = fixture("c2patool_two_manifests.wav");
    let asset = std::fs::read(&path).unwrap();
    let report = c2patool(&["-d", path.to_str().unwrap()]);

    let manifests = report["manifests"].as_object().unwrap();
    assert_eq!(manifests.len(), 2, "the fixture must hold two manifests");

    let store = C2paStore::from_wav(&asset).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();
    assert_eq!(
        claim.manifest_label(),
        report["active_manifest"].as_str().unwrap()
    );
    assert!(claim.verify_assertions(&store).unwrap().intact());
}

/// Both readers refuse the same bytes, by opposite routes. c2pa-rs resolves the claim's declared
/// URIs and errors when one does not resolve to a box it can match (`assertion missing`, exit 1, no
/// report at all). This crate enumerates the store's boxes and finds one the claim never named
/// (`assertion.undeclared`). The refusal agrees; the stated reason does not, and that is worth
/// knowing rather than glossing.
#[test]
#[ignore = "needs c2patool on PATH"]
fn c2patool_and_this_crate_both_refuse_an_undeclared_assertion() {
    let asset_path = fixture("poc_sample.aiff");
    let sidecar_path = fixture("undeclared_assertion_sidecar.c2pa");

    let output = Command::new("c2patool")
        .args([
            "-d",
            asset_path.to_str().unwrap(),
            "--external-manifest",
            sidecar_path.to_str().unwrap(),
        ])
        .output()
        .expect("c2patool must be on PATH to run the differential");
    assert!(!output.status.success(), "c2patool must reject the store");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("assertion missing"), "{stderr}");

    let store = C2paStore::from_sidecar(&std::fs::read(&sidecar_path).unwrap()).unwrap();
    let claim = C2paClaim::from_store(&store).unwrap();
    let integrity = claim.verify_assertions(&store).unwrap();
    assert!(!integrity.intact());
    assert_eq!(integrity.undeclared(), ["apw.smuggledxx"]);
}
