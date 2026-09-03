#![cfg(feature = "http")]
#![allow(clippy::panic)]
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use audio_provenance_core::SigningKey;
use audio_provenance_registry::{
    ContentHash, Fingerprint, HttpRegistryAuth, HttpRegistryBackend, HttpRegistryOptions,
    RegistryBackend, RegistryRecord, SignedAt, UnavailableKind, WritableRegistryBackend,
};

fn absent_content() -> ContentHash {
    ContentHash::from_content(b"nothing in any registry hashes to this")
}

/// A backend outage on a key that is genuinely absent must still be an outage.
#[test]
fn an_unreachable_host_is_not_a_miss_either() {
    let dead_port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        port
    };
    let remote = HttpRegistryBackend::new(
        "public",
        &format!("http://127.0.0.1:{dead_port}"),
        HttpRegistryOptions {
            max_retries: 0,
            timeout: Duration::from_millis(500),
            ..HttpRegistryOptions::default()
        },
    )
    .unwrap();
    let outcome = remote.lookup_by_content_hash(&absent_content());
    let reason = outcome.unavailable_reason().unwrap_or_else(|| {
        panic!("an unreachable host answered {outcome:?} instead of Unavailable")
    });
    assert!(matches!(
        reason.kind(),
        UnavailableKind::Transport | UnavailableKind::Timeout
    ));
}

fn serve(response: impl Fn(&mut TcpStream, &[u8]) + Send + 'static) -> (String, mpsc::Sender<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (stop, stopped) = mpsc::channel::<()>();
    thread::spawn(move || {
        while stopped.try_recv().is_err() {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut scratch = [0u8; 4096];
            while let Ok(read) = stream.read(&mut scratch) {
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&scratch[..read]);
                let header_end = request.windows(4).position(|bytes| bytes == b"\r\n\r\n");
                if let Some(header_end) = header_end {
                    let headers = String::from_utf8_lossy(&request[..header_end]).to_lowercase();
                    let content_length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            response(&mut stream, &request);
        }
    });
    (format!("http://127.0.0.1:{port}"), stop)
}

#[test]
fn the_http_backend_honours_its_timeout() {
    let (base, _stop) = serve(|_stream, _request| thread::sleep(Duration::from_secs(30)));
    let backend = HttpRegistryBackend::new(
        "public",
        &base,
        HttpRegistryOptions {
            timeout: Duration::from_millis(250),
            max_retries: 0,
            ..HttpRegistryOptions::default()
        },
    )
    .unwrap();

    let started = std::time::Instant::now();
    let outcome = backend.lookup_by_content_hash(&absent_content());
    let reason = outcome
        .unavailable_reason()
        .unwrap_or_else(|| panic!("a stalled server answered {outcome:?}"));
    assert_eq!(reason.kind(), UnavailableKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the request outlived its timeout by {:?}",
        started.elapsed()
    );
}

#[test]
fn the_http_backend_bounds_the_response_size() {
    let limit = 4096usize;

    let (declared_base, _declared_stop) = serve(move |stream, _request| {
        let body = vec![b'x'; limit * 4];
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(&body);
    });
    let backend = HttpRegistryBackend::new(
        "public",
        &declared_base,
        HttpRegistryOptions {
            max_response_bytes: limit,
            max_retries: 0,
            timeout: Duration::from_secs(5),
            ..HttpRegistryOptions::default()
        },
    )
    .unwrap();
    let outcome = backend.lookup_by_content_hash(&absent_content());
    assert_eq!(
        outcome
            .unavailable_reason()
            .unwrap_or_else(|| panic!("an oversized response answered {outcome:?}"))
            .kind(),
        UnavailableKind::ResponseTooLarge
    );

    // Chunked, so Content-Length cannot be trusted to bound the read.
    let (chunked_base, _chunked_stop) = serve(move |stream, _request| {
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        let chunk = vec![b'x'; 1024];
        for _ in 0..(limit / 1024 + 4) {
            if write!(stream, "{:x}\r\n", chunk.len()).is_err()
                || stream.write_all(&chunk).is_err()
                || stream.write_all(b"\r\n").is_err()
            {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n");
    });
    let backend = HttpRegistryBackend::new(
        "public",
        &chunked_base,
        HttpRegistryOptions {
            max_response_bytes: limit,
            max_retries: 0,
            timeout: Duration::from_secs(5),
            ..HttpRegistryOptions::default()
        },
    )
    .unwrap();
    let outcome = backend.lookup_by_content_hash(&absent_content());
    assert_eq!(
        outcome
            .unavailable_reason()
            .unwrap_or_else(|| panic!("an unbounded chunked response answered {outcome:?}"))
            .kind(),
        UnavailableKind::ResponseTooLarge
    );
}

fn signed_record() -> RegistryRecord {
    let key = SigningKey::from_raw_bytes(&[7; 32]).unwrap();
    let unsigned = serde_json::json!({
        "schema": "audio-provenance-manifest-v0",
        "locator_salt": "ab".repeat(16),
    });
    let signature = key.sign_manifest(&unsigned, "fixture.pub").unwrap();
    let mut manifest = unsigned;
    manifest["portable_signature"] = serde_json::to_value(signature).unwrap();
    RegistryRecord::from_signed_manifest(
        manifest,
        ContentHash::from_content(b"remote registry fixture"),
        Some(Fingerprint::new(vec![9; 32]).unwrap()),
        SignedAt::parse("2026-09-01T12:00:00Z").unwrap(),
    )
    .unwrap()
}

#[test]
fn authenticated_http_registry_publishes_an_idempotent_record_put() {
    let (captured_tx, captured_rx) = mpsc::channel();
    let (base, _stop) = serve(move |stream, request| {
        captured_tx.send(request.to_vec()).unwrap();
        let _ = stream.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n");
    });
    let backend = HttpRegistryBackend::new_authenticated(
        "production",
        &base,
        HttpRegistryOptions {
            max_retries: 0,
            ..HttpRegistryOptions::default()
        },
        HttpRegistryAuth::Bearer("registry-secret".to_owned()),
    )
    .unwrap();
    let debug = format!("{backend:?}");
    assert!(!debug.contains("registry-secret"));
    let record = signed_record();
    WritableRegistryBackend::put(&backend, &record).unwrap();

    let request = String::from_utf8_lossy(&captured_rx.recv().unwrap()).to_lowercase();
    assert!(request.starts_with(&format!(
        "put /v0/record/{} http/1.1",
        record.record_id().to_hex()
    )));
    assert!(request.contains("authorization: bearer registry-secret"));
    assert!(request.contains("content-type: application/json"));
    assert!(request.contains("audio-provenance-registry-record-v1"));
}
