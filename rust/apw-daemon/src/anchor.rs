//! RFC 3161 time anchoring over plain HTTP, the counterpart of
//! `daemon/time_anchor/anchor.py::RFC3161Provider` and `TimeAnchorService`.
//!
//! No HTTP client crate is in the dependency tree, so the exchange is a minimal
//! HTTP/1.1 POST from `crate::http`, over TLS for `https://` URLs. A redirect,
//! any non-200 reply or a failed handshake degrades to the explicit
//! `unavailable` record, exactly like any other failed exchange.

use std::time::Duration;

use apw_core::{
    anchored_record, encode_timestamp_request, unavailable_anchor_record, TimeProof,
    MAX_TSA_RESPONSE_BYTES, TSA_TIMEOUT_SECONDS,
};
use serde_json::Value;

use crate::http::{HttpClient, HttpRequest, Method};
use crate::services::TimeAnchor;

/// One request/response exchange with a TSA. Separate from the protocol so the
/// anchoring rules are testable without a network.
pub trait TsaTransport: Send + Sync {
    /// POST `request_der` and return at most `MAX_TSA_RESPONSE_BYTES + 1` body
    /// bytes, so the caller can tell an oversized reply from one at the bound.
    fn post(&self, url: &str, request_der: &[u8]) -> Result<Vec<u8>, String>;
}

pub struct HttpTransport {
    client: Result<HttpClient, String>,
}

impl HttpTransport {
    pub fn new() -> Self {
        HttpTransport { client: HttpClient::new() }
    }

    pub fn with_client(client: HttpClient) -> Self {
        HttpTransport { client: Ok(client) }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        HttpTransport::new()
    }
}

impl TsaTransport for HttpTransport {
    fn post(&self, url: &str, request_der: &[u8]) -> Result<Vec<u8>, String> {
        let client = self.client.as_ref().map_err(Clone::clone)?;
        let response = client.send(&HttpRequest {
            method: Method::Post,
            url,
            headers: &[
                ("Content-Type", "application/timestamp-query"),
                ("Accept", "application/timestamp-reply"),
            ],
            body: request_der,
            max_body: MAX_TSA_RESPONSE_BYTES,
            timeout: Duration::from_secs(TSA_TIMEOUT_SECONDS),
        })?;
        // Redirects are refused, not followed: a 3xx lands here as an error.
        if response.status != 200 {
            return Err(format!(
                "the TSA answered with HTTP status line {:?}",
                response.status_line
            ));
        }
        Ok(response.body)
    }
}

/// Anchors an export hash at an RFC 3161 TSA and renders the manifest record.
pub struct Rfc3161Anchor {
    tsa_url: String,
    transport: Box<dyn TsaTransport>,
}

impl Rfc3161Anchor {
    pub fn new(tsa_url: impl Into<String>) -> Self {
        Rfc3161Anchor::with_transport(tsa_url, Box::new(HttpTransport::new()))
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
