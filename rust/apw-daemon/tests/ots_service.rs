//! The OpenTimestamps service against the Python oracle's record vectors, plus the
//! calendar/explorer transport behaviour that only the daemon side owns.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::{Arc, Mutex};

use apw_core::{evaluate_ots_record, parse_detached, python_from_hex, HeaderSource};
use apw_daemon::{
    calendar_get, upgrade, CalendarTransport, ExplorerHeaderSource, HttpResponse, Method, OtsAnchor, TimeAnchor,
};
use serde_json::{json, Value};

mod common;

fn vectors() -> Value {
    common::parity_json("ots_vectors.json")
}

/// Answers from a URL table: `(status, body)` per base URL.
struct Table {
    replies: Vec<(String, u16, Vec<u8>)>,
    seen: Mutex<Vec<(String, String, Vec<u8>)>>,
}

impl CalendarTransport for Table {
    fn request(&self, method: Method, url: &str, body: &[u8], max_body: usize) -> Result<HttpResponse, String> {
        self.seen.lock().unwrap().push((format!("{method:?}"), url.to_owned(), body.to_vec()));
        let base = url
            .strip_suffix("/digest")
            .or_else(|| url.rsplit_once("/timestamp/").map(|(base, _)| base))
            .unwrap_or(url);
        let (_, status, reply) = self
            .replies
            .iter()
            .find(|(candidate, _, _)| candidate.trim_end_matches('/') == base)
            .ok_or_else(|| format!("no calendar at {url}"))?;
        Ok(HttpResponse {
            status: *status,
            status_line: format!("HTTP/1.1 {status} X"),
            body: reply.iter().copied().take(max_body + 1).collect(),
        })
    }
}

#[test]
fn records_match_the_python_service() {
    let vectors = vectors();
    let list = vectors["records"].as_array().unwrap();
    assert!(list.len() >= 8);
    for vector in list {
        let name = vector["name"].as_str().unwrap();
        let replies = vector["replies"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(url, reply)| {
                (
                    url.clone(),
                    u16::try_from(reply["status"].as_u64().unwrap()).unwrap(),
                    python_from_hex(reply["body_hex"].as_str().unwrap()).unwrap(),
                )
            })
            .collect();
        let table = Arc::new(Table { replies, seen: Mutex::new(Vec::new()) });
        let calendars: Vec<String> = vector["calendars"]
            .as_array()
            .unwrap()
            .iter()
            .map(|url| url.as_str().unwrap().to_owned())
            .collect();
        let anchor = OtsAnchor::with_transport(calendars, table);
        let nonce: [u8; 16] = python_from_hex(vector["nonce_hex"].as_str().unwrap()).unwrap().try_into().unwrap();
        let produced = anchor.anchor_record_with_nonce(vector["data_hash"].as_str().unwrap(), &nonce);
        assert_eq!(
            serde_json::to_string(&produced).unwrap(),
            serde_json::to_string(&vector["expected"]).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn a_produced_record_passes_the_verifier_check_and_hides_the_file_digest_from_calendars() {
    let data_hash = "ab".repeat(32);
    let reply = python_from_hex("f008010101010101010108" .to_owned().as_str()).unwrap();
    let mut body = reply;
    body.extend([0x00]);
    body.extend([0x83, 0xdf, 0xe3, 0x0d, 0x2e, 0xf9, 0x0c, 0x8e, 0x0f, 0x0e]);
    body.extend(b"https://a.test");
    let table = Arc::new(Table { replies: vec![("https://a.test".to_owned(), 200, body)], seen: Mutex::new(Vec::new()) });
    let anchor = OtsAnchor::with_transport(vec!["https://a.test".to_owned()], Arc::clone(&table) as Arc<dyn CalendarTransport>);
    let record = anchor.anchor_record(&data_hash);
    assert_eq!(record["status"], "pending", "{record}");
    assert_eq!(record["apw:proof_level"], "unknown_unobserved");
    assert!(record.get("timestamp").is_none() && record.get("timestamp_ms").is_none());
    let seen = table.seen.lock().unwrap();
    assert_eq!(seen[0].2.len(), 32);
    assert_ne!(seen[0].2, python_from_hex(&data_hash).unwrap(), "the calendar must not see the file digest");
    assert_eq!(apw_core::ots_hex(&seen[0].2), record["commitment_hex"].as_str().unwrap());
    let manifest = json!({"export": {"sha256": data_hash}, "time_anchor_opentimestamps": record});
    let findings = evaluate_ots_record(&manifest, None, None);
    assert_eq!(findings[0].1, "time_anchor_ots_pending", "{findings:?}");
}

#[test]
fn upgrade_contacts_only_allowed_calendars_and_merges_the_reply() {
    let proof_bytes = python_from_hex(
        vectors()["parse"]
            .as_array()
            .unwrap()
            .iter()
            .find(|vector| vector["name"] == "real:incomplete.txt.ots")
            .unwrap()["proof_hex"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut proof = parse_detached(&proof_bytes).unwrap();
    let uri = proof
        .timestamp
        .walk()
        .iter()
        .find(|(_, attestation)| attestation.kind == apw_core::AttestationKind::Pending)
        .map(|(_, attestation)| attestation.uri.clone())
        .unwrap();

    let table = Table { replies: vec![(uri.clone(), 404, Vec::new())], seen: Mutex::new(Vec::new()) };
    let skipped = upgrade(&mut proof, &["https://elsewhere.example".to_owned()], &table);
    assert_eq!(skipped[0]["status"], "skipped");
    assert!(table.seen.lock().unwrap().is_empty(), "a skipped calendar must not be contacted");
    assert_eq!(upgrade(&mut proof, std::slice::from_ref(&uri), &table)[0]["status"], "not_ready");

    // 200: prepend 0xaabb, sha256, then a Bitcoin attestation at height 5.
    let mut reply = Vec::new();
    reply.extend([0xf1, 0x02, 0xaa, 0xbb, 0x08, 0x00, 0x05, 0x88, 0x96, 0x0d, 0x73, 0xd7, 0x19, 0x01, 0x01, 0x05]);
    let table = Table { replies: vec![(uri.clone(), 200, reply)], seen: Mutex::new(Vec::new()) };
    assert_eq!(upgrade(&mut proof, &[uri], &table)[0]["status"], "upgraded");
    let kinds: Vec<Value> = apw_core::ots_attestation_summary(&proof.timestamp);
    assert!(kinds.iter().any(|entry| entry["type"] == "bitcoin" && entry["height"] == 5), "{kinds:?}");
    assert_eq!(parse_detached(&proof.serialize().unwrap()).unwrap().file_digest(), proof.file_digest());
}

#[test]
fn calendar_status_errors_use_the_python_wording() {
    let table = Table { replies: vec![("https://a.test".to_owned(), 500, Vec::new())], seen: Mutex::new(Vec::new()) };
    assert_eq!(
        calendar_get(&table, "https://a.test", &[1, 2]).unwrap_err(),
        "the calendar answered 'HTTP/1.1 500 X'"
    );
}

#[test]
fn explorer_source_returns_only_a_header_that_hashes_to_the_named_block() {
    let header_hex = vectors()["headers"][0]["header_hex"].as_str().unwrap().to_owned();
    let block_hash = vectors()["headers"][0]["block_hash"].as_str().unwrap().to_owned();
    let table = Arc::new(Table {
        replies: vec![
            ("https://explorer.test/api/block-height/358391".to_owned(), 200, block_hash.clone().into_bytes()),
            (format!("https://explorer.test/api/block/{block_hash}/header"), 200, header_hex.clone().into_bytes()),
            ("https://explorer.test/api/block-height/1".to_owned(), 200, "11".repeat(32).into_bytes()),
            (format!("https://explorer.test/api/block/{}/header", "11".repeat(32)), 200, header_hex.into_bytes()),
        ],
        seen: Mutex::new(Vec::new()),
    });
    // The table strips "/digest" and "/timestamp/..." only, so explorer URLs match whole.
    let source = ExplorerHeaderSource::with_transport("https://explorer.test/api/", table);
    assert_eq!(source.kind(), "explorer");
    assert_eq!(apw_core::ots_hex(&source.header_at(358_391).unwrap().block_hash()), block_hash);
    assert!(source.header_at(1).unwrap_err().contains("does not hash"));
    assert!(source.header_at(999).is_err());
}
