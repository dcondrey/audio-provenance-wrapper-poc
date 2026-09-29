//! OpenTimestamps calendars: submission, upgrade and an explorer header source.
//! The counterpart of the calendar half of `daemon/time_anchor/ots.py`; the proof
//! format itself lives in `apw_core::ots`.
//!
//! Calendar replies are untrusted input. They are size-bounded, parsed by the
//! bounded proof codec, and can only add attestations to the commitment the daemon
//! sent. Upgrade contacts only calendars the caller allowed: a proof's pending URIs
//! must not choose where this process connects.

use std::sync::Arc;
use std::time::Duration;

use apw_core::{
    check_bitcoin_attestations, ots_attestation_summary, ots_pending_record, ots_unavailable_record,
    parse_detached, parse_ots_timestamp, python_from_hex, python_repr, BlockHeader, DetachedProof,
    HeaderSource, Op, OtsTimestamp, CALENDAR_TIMEOUT_SECONDS, DEFAULT_CALENDARS,
    MAX_CALENDAR_RESPONSE_BYTES, OP_SHA256,
};
use serde_json::{json, Value};

use crate::http::{HttpClient, HttpRequest, HttpResponse, Method};
use crate::services::TimeAnchor;

const USER_AGENT: &str = "audio-provenance-daemon";
const CALENDAR_ACCEPT: &str = "application/vnd.opentimestamps.v1";

/// One request to a calendar or explorer. Separate from the protocol so the rules
/// are testable without a network.
pub trait CalendarTransport: Send + Sync {
    fn request(&self, method: Method, url: &str, body: &[u8], max_body: usize) -> Result<HttpResponse, String>;
}

pub struct HttpCalendarTransport {
    client: Result<HttpClient, String>,
}

impl HttpCalendarTransport {
    pub fn new() -> Self {
        HttpCalendarTransport { client: HttpClient::new() }
    }

    pub fn with_client(client: HttpClient) -> Self {
        HttpCalendarTransport { client: Ok(client) }
    }
}

impl Default for HttpCalendarTransport {
    fn default() -> Self {
        HttpCalendarTransport::new()
    }
}

impl CalendarTransport for HttpCalendarTransport {
    fn request(&self, method: Method, url: &str, body: &[u8], max_body: usize) -> Result<HttpResponse, String> {
        let client = self.client.as_ref().map_err(Clone::clone)?;
        client.send(&HttpRequest {
            method,
            url,
            headers: &[("Accept", CALENDAR_ACCEPT), ("User-Agent", USER_AGENT)],
            body,
            max_body,
            timeout: Duration::from_secs(CALENDAR_TIMEOUT_SECONDS),
        })
    }
}

fn status_error(response: &HttpResponse) -> String {
    format!(
        "the calendar answered {}",
        python_repr(&Value::String(response.status_line.clone()))
    )
}

/// POST the commitment to `<url>/digest`; the reply is a timestamp for it.
pub fn calendar_submit(
    transport: &dyn CalendarTransport,
    url: &str,
    digest: &[u8],
) -> Result<OtsTimestamp, String> {
    let endpoint = format!("{}/digest", url.trim_end_matches('/'));
    let response = transport.request(Method::Post, &endpoint, digest, MAX_CALENDAR_RESPONSE_BYTES)?;
    if response.status != 200 {
        return Err(status_error(&response));
    }
    if response.body.len() > MAX_CALENDAR_RESPONSE_BYTES {
        return Err("calendar response exceeded the size bound".to_owned());
    }
    parse_ots_timestamp(&response.body, digest).map_err(|error| error.to_string())
}

/// The calendar's timestamp for `commitment`, or `None` while it has none yet.
pub fn calendar_get(
    transport: &dyn CalendarTransport,
    url: &str,
    commitment: &[u8],
) -> Result<Option<OtsTimestamp>, String> {
    let endpoint = format!(
        "{}/timestamp/{}",
        url.trim_end_matches('/'),
        apw_core::ots_hex(commitment)
    );
    let response = transport.request(Method::Get, &endpoint, &[], MAX_CALENDAR_RESPONSE_BYTES)?;
    if response.status == 404 {
        return Ok(None);
    }
    if response.status != 200 {
        return Err(status_error(&response));
    }
    if response.body.len() > MAX_CALENDAR_RESPONSE_BYTES {
        return Err("calendar response exceeded the size bound".to_owned());
    }
    parse_ots_timestamp(&response.body, commitment)
        .map(Some)
        .map_err(|error| error.to_string())
}

/// Merge each allowed calendar's completed timestamp into `proof`, in place. One
/// outcome per pending attestation: upgraded, not_ready, skipped or failed.
pub fn upgrade(
    proof: &mut DetachedProof,
    allowed_calendars: &[String],
    transport: &dyn CalendarTransport,
) -> Vec<Value> {
    let allowed: Vec<&str> = allowed_calendars.iter().map(|url| url.trim_end_matches('/')).collect();
    let pending: Vec<(Vec<u8>, String)> = proof
        .timestamp
        .walk()
        .into_iter()
        .filter(|(_, attestation)| attestation.kind == apw_core::AttestationKind::Pending)
        .map(|(msg, attestation)| (msg.to_vec(), attestation.uri.clone()))
        .collect();
    let mut outcomes = Vec::new();
    for (msg, uri) in pending {
        let trimmed = uri.trim_end_matches('/');
        if !allowed.contains(&trimmed) {
            outcomes.push(json!({"uri": uri, "status": "skipped", "reason": "calendar not in the allowed set"}));
            continue;
        }
        match calendar_get(transport, trimmed, &msg) {
            Err(reason) => outcomes.push(json!({"uri": uri, "status": "failed", "reason": reason})),
            Ok(None) => outcomes.push(json!({
                "uri": uri, "status": "not_ready", "reason": "the calendar has no attestation yet"
            })),
            Ok(Some(fresh)) => {
                let merged = proof
                    .timestamp
                    .node_for(&msg)
                    .ok_or_else(|| "the pending node vanished".to_owned())
                    .and_then(|node| node.merge(&fresh).map_err(|error| error.to_string()));
                match merged {
                    Ok(()) => outcomes.push(json!({"uri": uri, "status": "upgraded"})),
                    Err(reason) => outcomes.push(json!({"uri": uri, "status": "failed", "reason": reason})),
                }
            }
        }
    }
    outcomes
}

/// Submits a hash to OpenTimestamps calendars and renders the manifest record.
pub struct OtsAnchor {
    calendars: Vec<String>,
    transport: Arc<dyn CalendarTransport>,
}

impl OtsAnchor {
    /// An empty list selects the default calendars.
    pub fn new(calendars: Vec<String>) -> Self {
        OtsAnchor::with_transport(calendars, Arc::new(HttpCalendarTransport::new()))
    }

    pub fn with_transport(calendars: Vec<String>, transport: Arc<dyn CalendarTransport>) -> Self {
        let calendars = if calendars.is_empty() {
            DEFAULT_CALENDARS.iter().map(|url| (*url).to_owned()).collect()
        } else {
            calendars
        };
        OtsAnchor { calendars, transport }
    }

    /// The record for `data_hash` with an explicit nonce (tests pin it; production
    /// draws 16 secure random bytes).
    pub fn anchor_record_with_nonce(&self, data_hash: &str, nonce: &[u8; 16]) -> Value {
        let digest = python_from_hex(data_hash).filter(|digest| digest.len() == 32);
        let Some(digest) = digest else {
            return ots_unavailable_record(
                data_hash,
                "data hash must be a 64-character SHA-256 hex digest",
                Vec::new(),
            );
        };
        let mut file_stamp = OtsTimestamp::new(digest);
        // REQUIRED: the calendars see only sha256(digest || nonce), never the file
        // digest, and the nonce keeps the commitment unpredictable.
        let commitment = match file_stamp
            .add_op(Op::append(nonce.to_vec()))
            .and_then(|stamp| stamp.add_op(Op::sha256()))
        {
            Ok(stamp) => stamp.msg.clone(),
            Err(error) => return ots_unavailable_record(data_hash, &error.to_string(), Vec::new()),
        };

        let results: Vec<(String, Result<OtsTimestamp, String>)> = std::thread::scope(|scope| {
            let handles: Vec<_> = self
                .calendars
                .iter()
                .map(|url| {
                    let commitment = &commitment;
                    let transport = self.transport.as_ref();
                    scope.spawn(move || (url.clone(), calendar_submit(transport, url, commitment)))
                })
                .collect();
            handles
                .into_iter()
                .zip(&self.calendars)
                .map(|(handle, url)| {
                    handle
                        .join()
                        .unwrap_or_else(|_| (url.clone(), Err("the calendar worker panicked".to_owned())))
                })
                .collect()
        });

        let mut outcomes = Vec::new();
        let mut merged_any = false;
        let mut merge_failure: Option<String> = None;
        for (url, result) in results {
            match result {
                Err(reason) => {
                    log::warn!("OpenTimestamps calendar {url} failed: {reason}");
                    outcomes.push(json!({"url": url, "status": "failed", "reason": reason}));
                }
                Ok(stamp) => {
                    let node = file_stamp
                        .node_for(&commitment)
                        .ok_or_else(|| "the commitment node vanished".to_owned())
                        .and_then(|node| node.merge(&stamp).map_err(|error| error.to_string()));
                    match node {
                        Ok(()) => {
                            merged_any = true;
                            outcomes.push(json!({"url": url, "status": "submitted"}));
                        }
                        Err(reason) => {
                            merge_failure = Some(reason.clone());
                            outcomes.push(json!({"url": url, "status": "failed", "reason": reason}));
                        }
                    }
                }
            }
        }
        if !merged_any {
            return ots_unavailable_record(
                data_hash,
                merge_failure.as_deref().unwrap_or("no calendar returned a usable timestamp"),
                outcomes,
            );
        }
        let proof = DetachedProof { file_hash_op: OP_SHA256, timestamp: file_stamp.clone() };
        match proof.serialize() {
            Ok(bytes) => ots_pending_record(data_hash, &commitment, outcomes, &file_stamp, &bytes),
            Err(error) => ots_unavailable_record(data_hash, &error.to_string(), outcomes),
        }
    }
}

impl TimeAnchor for OtsAnchor {
    fn anchor_record(&self, export_hash: &str) -> Value {
        let mut nonce = [0_u8; 16];
        if let Err(error) = getrandom::getrandom(&mut nonce) {
            let reason = format!("no secure randomness for the commitment nonce: {error}");
            log::warn!("OpenTimestamps anchoring failed: {reason}");
            return ots_unavailable_record(export_hash, &reason, Vec::new());
        }
        self.anchor_record_with_nonce(export_hash, &nonce)
    }
}

/// An Esplora-compatible explorer (`GET /block-height/<h>`, `GET /block/<hash>/header`).
/// The explorer is trusted to serve the best chain; this is not an independent check.
pub struct ExplorerHeaderSource {
    base_url: String,
    transport: Arc<dyn CalendarTransport>,
}

impl ExplorerHeaderSource {
    pub fn new(base_url: &str) -> Self {
        ExplorerHeaderSource::with_transport(base_url, Arc::new(HttpCalendarTransport::new()))
    }

    pub fn with_transport(base_url: &str, transport: Arc<dyn CalendarTransport>) -> Self {
        ExplorerHeaderSource { base_url: base_url.trim_end_matches('/').to_owned(), transport }
    }

    fn get(&self, path: &str, max_bytes: usize) -> Result<String, String> {
        let response = self
            .transport
            .request(Method::Get, &format!("{}{path}", self.base_url), &[], max_bytes)?;
        if response.status != 200 || response.body.len() > max_bytes {
            return Err(format!(
                "the explorer answered {} for {path}",
                python_repr(&Value::String(response.status_line))
            ));
        }
        Ok(String::from_utf8_lossy(&response.body).trim().to_owned())
    }
}

impl HeaderSource for ExplorerHeaderSource {
    fn kind(&self) -> &str {
        "explorer"
    }

    fn header_at(&self, height: u64) -> Result<BlockHeader, String> {
        let block_hash = self.get(&format!("/block-height/{height}"), 64)?;
        let header_hex = self.get(&format!("/block/{block_hash}/header"), 160)?;
        if block_hash.len() != 64 || header_hex.len() != 160 {
            return Err("the explorer returned a malformed hash or header".to_owned());
        }
        let raw = python_from_hex(&header_hex).ok_or("the explorer returned a malformed hash or header")?;
        let header = BlockHeader::new(&raw).map_err(|error| error.to_string())?;
        if apw_core::ots_hex(&header.block_hash()) != block_hash.to_ascii_lowercase() {
            return Err("the explorer's header does not hash to the block it named".to_owned());
        }
        Ok(header)
    }
}

/// Convenience for the CLI: verify every Bitcoin attestation in a proof file.
pub fn verify_proof_bytes(
    proof: &[u8],
    source: &dyn HeaderSource,
) -> Result<Vec<apw_core::BitcoinCheck>, String> {
    let parsed = parse_detached(proof).map_err(|error| error.to_string())?;
    Ok(check_bitcoin_attestations(&parsed.timestamp, source))
}

/// The attestations of a proof as the manifest records them.
pub fn summarize(proof: &DetachedProof) -> Vec<Value> {
    ots_attestation_summary(&proof.timestamp)
}
