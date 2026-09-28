//! RFC 3161 time anchoring over plain HTTP, the counterpart of
//! `daemon/time_anchor/anchor.py::RFC3161Provider` and `TimeAnchorService`.
//!
//! No HTTP client crate is in the dependency tree, so the exchange is a minimal
//! HTTP/1.1 POST over `std::net`. That covers `http://` TSAs, which is what the
//! default server is. An `https://` URL, a redirect or any non-200 reply degrades
//! to the explicit `unavailable` record, exactly like any other failed exchange.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use apw_core::{
    anchored_record, encode_timestamp_request, unavailable_anchor_record, TimeProof,
    MAX_TSA_RESPONSE_BYTES, TSA_TIMEOUT_SECONDS,
};
use serde_json::Value;

use crate::services::TimeAnchor;

/// One request/response exchange with a TSA. Separate from the protocol so the
/// anchoring rules are testable without a network.
pub trait TsaTransport: Send + Sync {
    /// POST `request_der` and return at most `MAX_TSA_RESPONSE_BYTES + 1` body
    /// bytes, so the caller can tell an oversized reply from one at the bound.
    fn post(&self, url: &str, request_der: &[u8]) -> Result<Vec<u8>, String>;
}

const MAX_HEADER_BYTES: usize = 16 * 1024;

pub struct HttpTransport;

struct HttpTarget {
    host: String,
    port: u16,
    path: String,
    host_header: String,
}

fn parse_http_url(url: &str) -> Result<HttpTarget, String> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(format!(
            "unsupported TSA URL scheme in {url:?}: this daemon speaks plain HTTP only"
        ));
    };
    let (authority, path) = match rest.find(['/', '?']) {
        Some(index) => {
            let (authority, tail) = rest.split_at(index);
            let path = if tail.starts_with('/') {
                tail.to_owned()
            } else {
                format!("/{tail}")
            };
            (authority, path)
        }
        None => (rest, "/".to_owned()),
    };
    if authority.is_empty() || authority.contains(['@', ' ', '\r', '\n']) || path.contains(['\r', '\n', ' ']) {
        return Err(format!("invalid TSA URL {url:?}"));
    }
    let port_split = if authority.starts_with('[') {
        authority.split_once("]:")
    } else {
        authority.rsplit_once(':')
    };
    let (host, port) = match port_split {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| format!("invalid TSA URL port in {url:?}"))?;
            (host, port)
        }
        None => (authority, 80),
    };
    Ok(HttpTarget {
        host: host.trim_start_matches('[').trim_end_matches(']').to_owned(),
        port,
        path,
        host_header: authority.to_owned(),
    })
}

impl TsaTransport for HttpTransport {
    fn post(&self, url: &str, request_der: &[u8]) -> Result<Vec<u8>, String> {
        let target = parse_http_url(url)?;
        let timeout = Duration::from_secs(TSA_TIMEOUT_SECONDS);
        let deadline = Instant::now() + timeout;
        let addresses = (target.host.as_str(), target.port)
            .to_socket_addrs()
            .map_err(|error| format!("cannot resolve the TSA host: {error}"))?;
        let mut last_error = "the TSA host has no addresses".to_owned();
        let mut stream = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, timeout) {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(error) => last_error = format!("cannot connect to the TSA: {error}"),
            }
        }
        let mut stream = stream.ok_or(last_error)?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|error| error.to_string())?;
        let head = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/timestamp-query\r\n\
             Content-Length: {}\r\nAccept: application/timestamp-reply\r\nConnection: close\r\n\r\n",
            target.path,
            target.host_header,
            request_der.len()
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(request_der))
            .and_then(|()| stream.flush())
            .map_err(|error| format!("cannot send the timestamp request: {error}"))?;
        read_reply(&mut stream, deadline)
    }
}

/// Read the reply under one overall deadline and a hard size bound. Keeps at
/// most `MAX_HEADER_BYTES + MAX_TSA_RESPONSE_BYTES + 1` bytes.
fn read_reply(stream: &mut TcpStream, deadline: Instant) -> Result<Vec<u8>, String> {
    let limit = MAX_HEADER_BYTES + MAX_TSA_RESPONSE_BYTES + 1;
    let mut received: Vec<u8> = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| "the TSA reply timed out".to_owned())?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|error| error.to_string())?;
        let read = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => return Err(format!("cannot read the TSA reply: {error}")),
        };
        received.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if received.len() >= limit {
            break;
        }
    }
    parse_reply(&received)
}

fn parse_reply(received: &[u8]) -> Result<Vec<u8>, String> {
    let split = received
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "the TSA reply has no complete HTTP header".to_owned())?;
    if split > MAX_HEADER_BYTES {
        return Err("the TSA reply header exceeds the size bound".to_owned());
    }
    let head = String::from_utf8_lossy(received.get(..split).unwrap_or_default()).into_owned();
    let body = received.get(split + 4..).unwrap_or_default();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line.split_whitespace().nth(1).unwrap_or_default();
    if status != "200" {
        return Err(format!("the TSA answered with HTTP status line {status_line:?}"));
    }
    let mut content_length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| "the TSA sent an invalid Content-Length".to_owned())?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    let payload = if chunked {
        decode_chunked(body)?
    } else {
        match content_length {
            // Reading stops at the size cap, so a body longer than the bound is
            // truncated to just past it; the caller reports it as oversized.
            Some(length) if body.len() < length && body.len() <= MAX_TSA_RESPONSE_BYTES => {
                return Err("the TSA reply body is shorter than its Content-Length".to_owned());
            }
            Some(length) => body.get(..length.min(body.len())).unwrap_or_default().to_vec(),
            None => body.to_vec(),
        }
    };
    Ok(payload.into_iter().take(MAX_TSA_RESPONSE_BYTES + 1).collect())
}

fn decode_chunked(mut body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| "the TSA sent a truncated chunked body".to_owned())?;
        let size_text = String::from_utf8_lossy(body.get(..line_end).unwrap_or_default()).into_owned();
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| "the TSA sent an invalid chunk size".to_owned())?;
        body = body.get(line_end + 2..).unwrap_or_default();
        if size == 0 {
            return Ok(out);
        }
        let chunk = body
            .get(..size)
            .ok_or_else(|| "the TSA sent a truncated chunked body".to_owned())?;
        out.extend_from_slice(chunk);
        if out.len() > MAX_TSA_RESPONSE_BYTES {
            return Ok(out);
        }
        body = body.get(size + 2..).unwrap_or_default();
    }
}

/// Anchors an export hash at an RFC 3161 TSA and renders the manifest record.
pub struct Rfc3161Anchor {
    tsa_url: String,
    transport: Box<dyn TsaTransport>,
}

impl Rfc3161Anchor {
    pub fn new(tsa_url: impl Into<String>) -> Self {
        Rfc3161Anchor::with_transport(tsa_url, Box::new(HttpTransport))
    }

    pub fn with_transport(tsa_url: impl Into<String>, transport: Box<dyn TsaTransport>) -> Self {
        Rfc3161Anchor {
            tsa_url: tsa_url.into(),
            transport,
        }
    }

    fn anchor(&self, data_hash: &str, nonce: &[u8]) -> Result<TimeProof, String> {
        let request = encode_timestamp_request(data_hash, nonce).map_err(|error| error.to_string())?;
        let body = self.transport.post(&self.tsa_url, &request)?;
        TimeProof::from_response(&self.tsa_url, data_hash, nonce, &body).map_err(|error| error.to_string())
    }
}

impl TimeAnchor for Rfc3161Anchor {
    /// Degrades to an explicit unavailable record instead of failing the manifest.
    fn anchor_record(&self, export_hash: &str) -> Value {
        // REQUIRED: the nonce is the replay protection, so it must be
        // unpredictable. A hash of known inputs would let an attacker pre-fetch a
        // backdated token carrying the predicted nonce. No weaker fallback.
        let mut nonce = [0_u8; 16];
        if let Err(error) = getrandom::getrandom(&mut nonce) {
            let reason = format!("no secure randomness for the timestamp nonce: {error}");
            log::warn!("Time anchoring failed: {reason}");
            return unavailable_anchor_record(export_hash, &reason);
        }
        match self.anchor(export_hash, &nonce) {
            Ok(proof) => anchored_record(export_hash, &proof),
            Err(reason) => {
                log::warn!("Time anchoring failed: {reason}");
                unavailable_anchor_record(export_hash, &reason)
            }
        }
    }
}
