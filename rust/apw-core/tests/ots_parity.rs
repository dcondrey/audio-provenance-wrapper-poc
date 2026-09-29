//! OpenTimestamps codec, header checks and record findings against
//! `tests/fixtures/parity/ots_vectors.json`. Real vectors are unmodified proofs from the
//! reference client's examples; constructed vectors are labelled `origin: constructed`
//! and were produced by the Python port, which is itself checked against the
//! reference implementation in `tests/test_ots.py`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use apw_core::{
    evaluate_ots_record, file_hash_op_name, ots_attestation_summary, ots_hex, parse_detached,
    python_from_hex, sha256_hex, BlockHeader, HeaderSource,
};
use serde_json::Value;

fn vectors() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/parity/ots_vectors.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn every_proof_parses_and_reserialises_like_the_oracle() {
    let vectors = vectors();
    let list = vectors["parse"].as_array().unwrap();
    assert!(list.len() >= 55, "the fixture lost its coverage");
    let mut real = 0;
    for vector in list {
        let name = vector["name"].as_str().unwrap();
        let data = python_from_hex(vector["proof_hex"].as_str().unwrap()).unwrap();
        if vector["origin"] == "real" {
            real += 1;
        }
        let parsed = parse_detached(&data);
        match (vector.get("ok"), vector.get("error")) {
            (Some(ok), None) => {
                let proof = parsed.unwrap_or_else(|error| panic!("{name}: rejected: {error}"));
                assert_eq!(file_hash_op_name(proof.file_hash_op), ok["file_hash_op"].as_str().unwrap(), "{name}");
                assert_eq!(ots_hex(proof.file_digest()), ok["digest"].as_str().unwrap(), "{name}");
                assert_eq!(
                    Value::Array(ots_attestation_summary(&proof.timestamp)),
                    ok["attestations"],
                    "{name}"
                );
                let mut messages: Vec<String> = proof.timestamp.messages().into_iter().map(ots_hex).collect();
                messages.sort();
                messages.dedup();
                let expected: Vec<String> = ok["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|message| message.as_str().unwrap().to_owned())
                    .collect();
                assert_eq!(messages, expected, "{name}");
                let bytes = proof.serialize().unwrap();
                assert_eq!(sha256_hex(&bytes), ok["reserialized_sha256"].as_str().unwrap(), "{name}");
                assert_eq!(bytes == data, ok["reserialized_equals_input"].as_bool().unwrap(), "{name}");
            }
            (None, Some(error)) => {
                let message = parsed.err().unwrap_or_else(|| panic!("{name}: accepted a proof Python rejects"));
                assert_eq!(Some(message.0.as_str()), error.as_str(), "{name}");
            }
            _ => panic!("{name}: malformed fixture"),
        }
    }
    assert!(real >= 12);
}

#[test]
fn header_checks_match_the_oracle() {
    for vector in vectors()["headers"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        let raw = python_from_hex(vector["header_hex"].as_str().unwrap()).unwrap();
        let header = BlockHeader::new(&raw).unwrap();
        assert_eq!(header.meets_own_target(), vector["meets_own_target"].as_bool().unwrap(), "{name}");
        if let Some(hash) = vector.get("block_hash").and_then(Value::as_str) {
            assert_eq!(ots_hex(&header.block_hash()), hash, "{name}");
            assert_eq!(ots_hex(header.merkle_root()), vector["merkle_root"].as_str().unwrap(), "{name}");
            assert_eq!(u64::from(header.time()), vector["time"].as_u64().unwrap(), "{name}");
        }
    }
    assert!(BlockHeader::new(&[0_u8; 79]).is_err());
}

struct Table {
    kind: String,
    headers: Vec<(u64, BlockHeader)>,
}

impl HeaderSource for Table {
    fn kind(&self) -> &str {
        &self.kind
    }

    fn header_at(&self, height: u64) -> Result<BlockHeader, String> {
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == height)
            .map(|(_, header)| header.clone())
            .ok_or_else(|| format!("no header for height {height}"))
    }
}

#[test]
fn record_findings_match_the_oracle() {
    let vectors = vectors();
    let list = vectors["evaluate"].as_array().unwrap();
    assert!(list.len() >= 30);
    for vector in list {
        let name = vector["name"].as_str().unwrap();
        let source = vector.get("header_source").filter(|source| !source.is_null()).map(|source| Table {
            kind: source["kind"].as_str().unwrap().to_owned(),
            headers: source["headers"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(height, raw)| {
                    let raw = python_from_hex(raw.as_str().unwrap()).unwrap();
                    (height.parse().unwrap(), BlockHeader::new(&raw).unwrap())
                })
                .collect(),
        });
        let override_bytes = vector["override_hex"].as_str().map(|hex| python_from_hex(hex).unwrap());
        let findings = evaluate_ots_record(
            &vector["manifest"],
            source.as_ref().map(|table| table as &dyn HeaderSource),
            override_bytes.as_deref(),
        );
        let expected: Vec<(String, String, String)> = vector["expected_findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|finding| {
                (
                    finding["severity"].as_str().unwrap().to_owned(),
                    finding["code"].as_str().unwrap().to_owned(),
                    finding["message"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(findings, expected, "{name}");
    }
}
