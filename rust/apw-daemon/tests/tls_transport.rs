//! https transport against a local rustls server with a self-signed certificate.
//! The certificate is injected as a trusted root only here; production code has no
//! way to do that, and validation stays on in every case below.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use apw_daemon::{HttpClient, HttpRequest, HttpTransport, Method, Rfc3161Anchor, TimeAnchor, TsaTransport};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

const HASH: &str = "0ad6383cc78f41a0f6f75fcfa9c1bc237969ea5e669251a931370e7489fad161";

struct Server {
    port: u16,
    root: CertificateDer<'static>,
    handle: thread::JoinHandle<()>,
}

/// Serves `connections` TLS connections, answering each with `reply(request)`.
fn serve(names: &[&str], connections: usize, reply: fn(&[u8]) -> Vec<u8>) -> Server {
    let certified = rcgen::generate_simple_self_signed(
        names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>(),
    )
    .unwrap();
    let cert = certified.cert.der().clone();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(certified.signing_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = Arc::new(
        ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = thread::spawn(move || {
        for _ in 0..connections {
            let (tcp, _) = listener.accept().unwrap();
            let mut tls = StreamOwned::new(ServerConnection::new(Arc::clone(&config)).unwrap(), tcp);
            let mut received = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                match tls.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => received.extend_from_slice(&chunk[..read]),
                }
                if let Some(split) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&received[..split]).to_ascii_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(0);
                    if received.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            // A failed handshake (an untrusted client) ends here with no reply.
            let _ = tls.write_all(&reply(&received));
            let _ = tls.flush();
            tls.conn.send_close_notify();
            let _ = tls.flush();
        }
    });
    Server { port, root: cert, handle }
}

fn ok_reply(_request: &[u8]) -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello".to_vec()
}

fn redirect_reply(_request: &[u8]) -> Vec<u8> {
    b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        .to_vec()
}

fn get(client: &HttpClient, url: &str) -> Result<apw_daemon::HttpResponse, String> {
    client.send(&HttpRequest {
        method: Method::Get,
        url,
        headers: &[],
        body: &[],
        max_body: 100,
        timeout: std::time::Duration::from_secs(5),
    })
}

#[test]
fn a_trusted_self_signed_root_is_accepted_when_injected() {
    let server = serve(&["localhost"], 1, ok_reply);
    let client = HttpClient::with_additional_roots(vec![server.root.clone()]).unwrap();
    let response = get(&client, &format!("https://localhost:{}/ok", server.port)).unwrap();
    assert_eq!((response.status, response.body.as_slice()), (200, &b"hello"[..]));
    server.handle.join().unwrap();
}

#[test]
fn certificate_validation_is_on_by_default() {
    let server = serve(&["localhost"], 1, ok_reply);
    let error = get(&HttpClient::new().unwrap(), &format!("https://localhost:{}/ok", server.port))
        .unwrap_err();
    assert!(
        error.to_lowercase().contains("certificate") || error.contains("UnknownIssuer"),
        "an untrusted self-signed certificate must fail validation, got: {error}"
    );
    server.handle.join().unwrap();
}

#[test]
fn a_trusted_root_does_not_excuse_a_hostname_mismatch() {
    let server = serve(&["other.example"], 1, ok_reply);
    let client = HttpClient::with_additional_roots(vec![server.root.clone()]).unwrap();
    let error = get(&client, &format!("https://localhost:{}/ok", server.port)).unwrap_err();
    assert!(
        error.to_lowercase().contains("certificate") || error.contains("NotValidForName"),
        "{error}"
    );
    server.handle.join().unwrap();
}

#[test]
fn https_redirects_are_returned_not_followed() {
    let server = serve(&["localhost"], 1, redirect_reply);
    let client = HttpClient::with_additional_roots(vec![server.root.clone()]).unwrap();
    let response = get(&client, &format!("https://localhost:{}/r", server.port)).unwrap();
    assert_eq!(response.status, 302);
    server.handle.join().unwrap();

    // Through the TSA transport a 3xx is an error naming the status line.
    let server = serve(&["localhost"], 1, redirect_reply);
    let transport =
        HttpTransport::with_client(HttpClient::with_additional_roots(vec![server.root.clone()]).unwrap());
    let error = transport.post(&format!("https://localhost:{}/r", server.port), b"x").unwrap_err();
    assert!(error.contains("302"), "{error}");
    server.handle.join().unwrap();
}

/// A TSA reply is not needed to prove the anchor path speaks TLS: an https TSA that
/// answers with garbage degrades to the unavailable record, never to an anchor.
#[test]
fn an_https_tsa_that_answers_garbage_yields_an_unavailable_record() {
    let server = serve(&["localhost"], 1, ok_reply);
    let anchor = Rfc3161Anchor::with_transport(
        format!("https://localhost:{}/ts", server.port),
        Box::new(HttpTransport::with_client(
            HttpClient::with_additional_roots(vec![server.root.clone()]).unwrap(),
        )),
    );
    let record = anchor.anchor_record(HASH);
    assert_eq!(record["status"], "unavailable");
    assert_eq!(record["apw:proof_level"], "unknown_unobserved");
    server.handle.join().unwrap();
}
