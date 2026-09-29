//! The anchoring service and its HTTP transport. Protocol bytes are checked
//! against the Python oracle in `apw-core/tests/time_anchor_parity.rs`; this file
//! checks the behaviour that only the daemon side owns.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use apw_core::{check_time_anchor, parse_timestamp_request, VerificationReport, MAX_TSA_RESPONSE_BYTES};
use apw_daemon::{HttpTransport, Rfc3161Anchor, TimeAnchor, TsaTransport};

const HASH: &str = "0ad6383cc78f41a0f6f75fcfa9c1bc237969ea5e669251a931370e7489fad161";

fn digicert_response() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/rfc3161_digicert_response.hex");
    apw_core::python_from_hex(std::fs::read_to_string(path).unwrap().trim()).unwrap()
}

/// A transport that answers with a fixed body and records what it was sent.
struct Canned {
    reply: Result<Vec<u8>, String>,
    seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl TsaTransport for Canned {
    fn post(&self, _url: &str, request_der: &[u8]) -> Result<Vec<u8>, String> {
        self.seen.lock().unwrap().push(request_der.to_vec());
        self.reply.clone()
    }
}

fn anchor_with(reply: Result<Vec<u8>, String>) -> (Rfc3161Anchor, Arc<Mutex<Vec<Vec<u8>>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let anchor = Rfc3161Anchor::with_transport(
        "http://tsa.test/ts",
        Box::new(Canned { reply, seen: Arc::clone(&seen) }),
    );
    (anchor, seen)
}

#[test]
fn a_transport_failure_degrades_to_an_unavailable_record_that_asserts_no_time() {
    let (anchor, _) = anchor_with(Err("cannot connect to the TSA: refused".to_owned()));
    let record = anchor.anchor_record(HASH);
    assert_eq!(record["status"], "unavailable");
    assert_eq!(record["data_hash"], HASH);
    assert_eq!(record["reason"], "cannot connect to the TSA: refused");
    assert_eq!(record["apw:proof_level"], "unknown_unobserved");
    assert!(record.get("timestamp_ms").is_none(), "no local-clock time may appear");
    let manifest = serde_json::json!({"time_anchor": record, "export": {"sha256": HASH}});
    let mut report = VerificationReport::new();
    check_time_anchor(&manifest, &mut report);
    assert_eq!(report.findings[0].code, "time_anchor_unavailable");
}

#[test]
fn a_token_for_a_different_request_never_becomes_an_anchor() {
    // The DigiCert fixture carries a fixed nonce; the anchor draws a fresh one.
    let (anchor, seen) = anchor_with(Ok(digicert_response()));
    let record = anchor.anchor_record(HASH);
    assert_eq!(record["status"], "unavailable");
    assert_eq!(record["reason"], "TSA token nonce does not match the request nonce");
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let (hash, nonce) = parse_timestamp_request(&requests[0]).unwrap();
    assert_eq!(hash, HASH);
    assert!(!nonce.is_empty());
}

#[test]
fn nonces_are_fresh_per_request() {
    let (anchor, seen) = anchor_with(Err("x".to_owned()));
    anchor.anchor_record(HASH);
    anchor.anchor_record(HASH);
    let requests = seen.lock().unwrap();
    let nonces: Vec<Vec<u8>> = requests
        .iter()
        .map(|request| parse_timestamp_request(request).unwrap().1)
        .collect();
    assert_ne!(nonces[0], nonces[1]);
    // 16 random bytes; a leading zero byte may be stripped from the magnitude.
    assert!(nonces.iter().all(|nonce| (14..=16).contains(&nonce.len())));
}

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if content.len() < 0x80 {
        out.push(content.len() as u8);
    } else {
        out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
    }
    out.extend_from_slice(content);
    out
}

fn der_uint(magnitude: &[u8]) -> Vec<u8> {
    let mut body: Vec<u8> = magnitude.iter().copied().skip_while(|b| *b == 0).collect();
    if body.first().is_none_or(|b| b & 0x80 != 0) {
        body.insert(0, 0);
    }
    der(0x02, &body)
}

/// A granted TimeStampResp bound to the request's hash and nonce.
fn echoing_reply(request: &[u8]) -> Vec<u8> {
    let (hash, nonce) = parse_timestamp_request(request).unwrap();
    let sha256 = [0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    let mut algorithm = sha256.to_vec();
    algorithm.extend(der(0x05, &[]));
    let mut imprint = der(0x30, &algorithm);
    imprint.extend(der(0x04, &apw_core::python_from_hex(&hash).unwrap()));
    let mut tst = der(0x02, &[1]);
    tst.extend([0x06, 0x03, 0x2a, 0x03, 0x04]);
    tst.extend(der(0x30, &imprint));
    tst.extend(der_uint(&[9, 9]));
    tst.extend(der(0x18, b"20260928123000Z"));
    tst.extend(der_uint(&nonce));
    let mut encap = vec![0x06, 0x0b, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x01, 0x04];
    encap.extend(der(0xA0, &der(0x04, &der(0x30, &tst))));
    let mut signed = der_uint(&[3]);
    signed.extend(der(0x31, &[]));
    signed.extend(der(0x30, &encap));
    let mut token = vec![0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
    token.extend(der(0xA0, &der(0x30, &signed)));
    let mut response = der(0x30, &der_uint(&[0]));
    response.extend(der(0x30, &token));
    der(0x30, &response)
}

struct Echo;

impl TsaTransport for Echo {
    fn post(&self, _url: &str, request_der: &[u8]) -> Result<Vec<u8>, String> {
        Ok(echoing_reply(request_der))
    }
}

#[test]
fn an_echoed_token_anchors_at_inferred_and_the_verifier_agrees() {
    let anchor = Rfc3161Anchor::with_transport("http://tsa.test/ts", Box::new(Echo));
    let record = anchor.anchor_record(HASH);
    assert_eq!(record["status"], "anchored", "{record}");
    assert_eq!(record["apw:proof_level"], "inferred");
    assert_eq!(record["cms_signature_verified"], false);
    assert_eq!(record["source"], "rfc3161:http://tsa.test/ts");
    assert_eq!(record["timestamp_ms"], 1_790_598_600_000_i64);
    assert_eq!(record["timestamp"], "2026-09-28T12:30:00Z");
    let manifest = serde_json::json!({"time_anchor": record, "export": {"sha256": HASH}});
    let mut report = VerificationReport::new();
    check_time_anchor(&manifest, &mut report);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].code, "time_anchor_consistent");
}

// ---- HttpTransport over a loopback socket ----------------------------------------

fn serve_once(reply: Vec<u8>) -> (String, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            received.extend_from_slice(&chunk[..read]);
            if let Some(split) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&received[..split]).to_ascii_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if received.len() >= split + 4 + length {
                    break;
                }
            }
        }
        stream.write_all(&reply).unwrap();
        received
    });
    (format!("http://{address}/ts?x=1"), handle)
}

fn http_reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut reply = format!("HTTP/1.1 {status}\r\n{headers}Connection: close\r\n\r\n").into_bytes();
    reply.extend_from_slice(body);
    reply
}

#[test]
fn http_transport_posts_a_timestamp_query_and_returns_the_body() {
    let body = b"\x30\x03\x02\x01\x00".to_vec();
    let (url, server) = serve_once(http_reply(
        "200 OK",
        &format!("Content-Type: application/timestamp-reply\r\nContent-Length: {}\r\n", body.len()),
        &body,
    ));
    assert_eq!(HttpTransport::new().post(&url, b"REQUEST").unwrap(), body);
    let received = String::from_utf8(server.join().unwrap()).unwrap();
    assert!(received.starts_with("POST /ts?x=1 HTTP/1.1\r\n"), "{received}");
    assert!(received.contains("Content-Type: application/timestamp-query\r\n"));
    assert!(received.ends_with("\r\n\r\nREQUEST"));
}

#[test]
fn http_transport_decodes_chunked_bodies() {
    let (url, server) = serve_once(http_reply(
        "200 OK",
        "Transfer-Encoding: chunked\r\n",
        b"3\r\nabc\r\n2;ext=1\r\nde\r\n0\r\n\r\n",
    ));
    assert_eq!(HttpTransport::new().post(&url, b"x").unwrap(), b"abcde");
    server.join().unwrap();
}

#[test]
fn http_transport_bounds_the_body_it_returns() {
    let big = vec![7_u8; MAX_TSA_RESPONSE_BYTES * 2];
    let (url, server) = serve_once(http_reply(
        "200 OK",
        &format!("Content-Length: {}\r\n", big.len()),
        &big,
    ));
    let body = HttpTransport::new().post(&url, b"x").unwrap();
    assert_eq!(body.len(), MAX_TSA_RESPONSE_BYTES + 1, "one byte over the bound proves oversize");
    server.join().unwrap();
}

#[test]
fn http_transport_refuses_redirects_errors_and_unsupported_schemes() {
    let (url, server) = serve_once(http_reply("302 Found", "Location: http://elsewhere/\r\nContent-Length: 0\r\n", b""));
    assert!(HttpTransport::new().post(&url, b"x").unwrap_err().contains("302"));
    server.join().unwrap();

    let (url, server) = serve_once(http_reply("500 Oops", "Content-Length: 0\r\n", b""));
    assert!(HttpTransport::new().post(&url, b"x").unwrap_err().contains("500"));
    server.join().unwrap();

    let (url, server) = serve_once(http_reply("200 OK", "Content-Length: 50\r\n", b"short"));
    assert!(HttpTransport::new().post(&url, b"x").unwrap_err().contains("shorter"));
    server.join().unwrap();

    assert!(HttpTransport::new().post("ftp://tsa.example/ts", b"x").unwrap_err().contains("only http and https"));
    assert!(HttpTransport::new().post("http://", b"x").is_err());
    assert!(HttpTransport::new().post("http://host:notaport/", b"x").is_err());
    assert!(HttpTransport::new().post("http://ho st/", b"x").is_err());
}
