use std::fmt;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::backend::{RegistryBackend, RegistryKind, RegistrySource, WritableRegistryBackend};
use crate::error::RegistryError;
use crate::ids::{ContentHash, Fingerprint, MarkId, RecordId};
use crate::lookup::{Lookup, Unavailable, UnavailableKind};
use crate::record::{
    AdvisoryScore, MAX_FINGERPRINT_CANDIDATES, MAX_MARK_MATCHES, MarkMatches, RegistryRecord,
    ScoredCandidate,
};

/// The request/response shape this backend speaks, under whatever base URL the operator
/// configured.
///
/// IMPORTANT: this is this crate's own convention. No public Audio Provenance registry endpoint is
/// published, so there is no vendor API to be compatible with and no default host to fall back
/// on. Every URL below is built from a base the caller supplied.
pub const PROTOCOL_ID: &str = "audio-provenance-registry-http-v0";

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_RETRIES: u32 = 2;
pub const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_millis(250);

pub const MAX_TIMEOUT: Duration = Duration::from_secs(300);
pub const MAX_RESPONSE_BYTES_LIMIT: usize = 64 * 1024 * 1024;
pub const MAX_RETRIES_LIMIT: u32 = 8;
const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRegistryOptions {
    pub timeout: Duration,
    pub max_response_bytes: usize,
    pub max_retries: u32,
    pub retry_backoff: Duration,
}

impl Default for HttpRegistryOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            max_retries: DEFAULT_MAX_RETRIES,
            retry_backoff: DEFAULT_RETRY_BACKOFF,
        }
    }
}

impl HttpRegistryOptions {
    fn validate(&self) -> Result<(), RegistryError> {
        if self.timeout.is_zero() || self.timeout > MAX_TIMEOUT {
            return Err(RegistryError::OptionRange {
                field: "timeout_ms",
                limit: MAX_TIMEOUT.as_millis() as u64,
                found: self.timeout.as_millis() as u64,
            });
        }
        if self.max_response_bytes == 0 || self.max_response_bytes > MAX_RESPONSE_BYTES_LIMIT {
            return Err(RegistryError::OptionRange {
                field: "max_response_bytes",
                limit: MAX_RESPONSE_BYTES_LIMIT as u64,
                found: self.max_response_bytes as u64,
            });
        }
        if self.max_retries > MAX_RETRIES_LIMIT {
            return Err(RegistryError::OptionRange {
                field: "max_retries",
                limit: u64::from(MAX_RETRIES_LIMIT),
                found: u64::from(self.max_retries),
            });
        }
        Ok(())
    }
}

/// Authentication for a remote registry. Credential values are never included in `Debug`.
#[derive(Clone)]
pub enum HttpRegistryAuth {
    Bearer(String),
    CloudflareAccess {
        client_id: String,
        client_secret: String,
    },
}

impl fmt::Debug for HttpRegistryAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bearer(_) => f.write_str("Bearer([REDACTED])"),
            Self::CloudflareAccess { .. } => {
                f.write_str("CloudflareAccess { client_id: [REDACTED], client_secret: [REDACTED] }")
            }
        }
    }
}

impl HttpRegistryAuth {
    fn validate(&self) -> Result<(), RegistryError> {
        let valid = |value: &str| {
            !value.is_empty()
                && value.len() <= 4096
                && value.is_ascii()
                && !value.bytes().any(|byte| byte.is_ascii_control())
        };
        let accepted = match self {
            Self::Bearer(token) => valid(token),
            Self::CloudflareAccess {
                client_id,
                client_secret,
            } => valid(client_id) && valid(client_secret),
        };
        if accepted {
            Ok(())
        } else {
            Err(RegistryError::RemotePublish {
                detail:
                    "registry credentials must be non-empty printable ASCII and at most 4096 bytes"
                        .to_owned(),
            })
        }
    }
}

/// A registry reached over HTTP at an explicitly configured base URL.
pub struct HttpRegistryBackend {
    source: RegistrySource,
    base_url: String,
    options: HttpRegistryOptions,
    agent: ureq::Agent,
    auth: Option<HttpRegistryAuth>,
}

impl fmt::Debug for HttpRegistryBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRegistryBackend")
            .field("name", &self.source.name())
            .field("base_url", &self.base_url)
            .field("options", &self.options)
            .field("auth", &self.auth)
            .finish()
    }
}

/// Reject anything that could smuggle credentials, redirect the request elsewhere, or make the
/// resolved path depend on server-controlled text.
fn validate_base_url(base_url: &str) -> Result<String, RegistryError> {
    if base_url.is_empty() || base_url.len() > 512 || !base_url.is_ascii() {
        return Err(RegistryError::BaseUrl {
            reason: "must be 1..=512 ASCII characters",
        });
    }
    if base_url
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return Err(RegistryError::BaseUrl {
            reason: "must not contain whitespace or control characters",
        });
    }
    let uri: ureq::http::Uri = base_url.parse().map_err(|_| RegistryError::BaseUrl {
        reason: "is not a valid absolute URI",
    })?;
    match uri.scheme_str() {
        Some("http" | "https") => {}
        _ => {
            return Err(RegistryError::BaseUrl {
                reason: "must use the http or https scheme",
            });
        }
    }
    let authority = uri.authority().ok_or(RegistryError::BaseUrl {
        reason: "must name a host",
    })?;
    if authority.as_str().contains('@') {
        return Err(RegistryError::BaseUrl {
            reason: "must not embed credentials in the authority",
        });
    }
    if uri.query().is_some() {
        return Err(RegistryError::BaseUrl {
            reason: "must not carry a query string",
        });
    }
    if uri.path().contains("..") {
        return Err(RegistryError::BaseUrl {
            reason: "must not contain a relative path segment",
        });
    }
    Ok(base_url.trim_end_matches('/').into())
}

enum Attempt {
    Body(Vec<u8>),
    Missing,
    Retry(Unavailable),
    Fatal(Unavailable),
}

impl HttpRegistryBackend {
    /// There is no default base URL and no fallback guess. The caller supplies one.
    pub fn new(
        name: impl Into<String>,
        base_url: &str,
        options: HttpRegistryOptions,
    ) -> Result<Self, RegistryError> {
        Self::new_inner(name, base_url, options, None)
    }

    /// Creates a registry backend that authenticates reads and supports idempotent publication.
    pub fn new_authenticated(
        name: impl Into<String>,
        base_url: &str,
        options: HttpRegistryOptions,
        auth: HttpRegistryAuth,
    ) -> Result<Self, RegistryError> {
        auth.validate()?;
        Self::new_inner(name, base_url, options, Some(auth))
    }

    fn new_inner(
        name: impl Into<String>,
        base_url: &str,
        options: HttpRegistryOptions,
        auth: Option<HttpRegistryAuth>,
    ) -> Result<Self, RegistryError> {
        options.validate()?;
        let base_url = validate_base_url(base_url)?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(options.timeout))
            .max_response_header_size(MAX_RESPONSE_HEADER_BYTES)
            .http_status_as_error(false)
            // No ambient credentials: proxy environment variables can carry user:password, and a
            // redirect can move the request to a host the operator never configured.
            .proxy(None)
            .max_redirects(0)
            .build();
        let source = RegistrySource::new(name, RegistryKind::Http, base_url.clone());
        Ok(Self {
            source,
            base_url,
            options,
            agent: config.into(),
            auth,
        })
    }

    pub const fn options(&self) -> &HttpRegistryOptions {
        &self.options
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn attempt(&self, url: &str, body: Option<&[u8]>) -> Attempt {
        let response = match body {
            None => {
                let request = self.agent.get(url).header("accept", "application/json");
                match &self.auth {
                    None => request.call(),
                    Some(HttpRegistryAuth::Bearer(token)) => request
                        .header("authorization", &format!("Bearer {token}"))
                        .call(),
                    Some(HttpRegistryAuth::CloudflareAccess {
                        client_id,
                        client_secret,
                    }) => request
                        .header("cf-access-client-id", client_id)
                        .header("cf-access-client-secret", client_secret)
                        .call(),
                }
            }
            Some(payload) => {
                let request = self
                    .agent
                    .post(url)
                    .header("accept", "application/json")
                    .header("content-type", "application/json");
                match &self.auth {
                    None => request.send(payload),
                    Some(HttpRegistryAuth::Bearer(token)) => request
                        .header("authorization", &format!("Bearer {token}"))
                        .send(payload),
                    Some(HttpRegistryAuth::CloudflareAccess {
                        client_id,
                        client_secret,
                    }) => request
                        .header("cf-access-client-id", client_id)
                        .header("cf-access-client-secret", client_secret)
                        .send(payload),
                }
            }
        };
        let mut response = match response {
            Ok(response) => response,
            Err(ureq::Error::Timeout(_)) => {
                return Attempt::Retry(Unavailable::new(
                    UnavailableKind::Timeout,
                    format!("request to {url} exceeded {:?}", self.options.timeout),
                ));
            }
            Err(error) => {
                return Attempt::Retry(Unavailable::new(
                    UnavailableKind::Transport,
                    format!("request to {url} failed: {error}"),
                ));
            }
        };

        let status = response.status().as_u16();
        match status {
            200 => {}
            404 => return Attempt::Missing,
            401 | 403 => {
                return Attempt::Fatal(Unavailable::new(
                    UnavailableKind::Unauthorized,
                    format!("{url} answered {status}; this is not evidence of absence"),
                ));
            }
            429 => {
                return Attempt::Retry(Unavailable::new(
                    UnavailableKind::RateLimited,
                    format!("{url} answered 429"),
                ));
            }
            500..=599 => {
                return Attempt::Retry(Unavailable::new(
                    UnavailableKind::ServerError,
                    format!("{url} answered {status}"),
                ));
            }
            _ => {
                return Attempt::Fatal(Unavailable::new(
                    UnavailableKind::MalformedResponse,
                    format!("{url} answered an unexpected status {status}"),
                ));
            }
        }

        // Two gates: the declared length, then the bytes actually read. A server can understate
        // Content-Length, so the read is bounded independently.
        if let Some(declared) = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            && declared > self.options.max_response_bytes
        {
            return Attempt::Fatal(Unavailable::new(
                UnavailableKind::ResponseTooLarge,
                format!(
                    "{url} declared {declared} bytes, over the {} byte limit",
                    self.options.max_response_bytes
                ),
            ));
        }

        match response
            .body_mut()
            .with_config()
            .limit(self.options.max_response_bytes as u64)
            .read_to_vec()
        {
            Ok(bytes) if bytes.len() <= self.options.max_response_bytes => Attempt::Body(bytes),
            Ok(_) => Attempt::Fatal(Unavailable::new(
                UnavailableKind::ResponseTooLarge,
                format!(
                    "{url} returned more than the {} byte limit",
                    self.options.max_response_bytes
                ),
            )),
            Err(ureq::Error::BodyExceedsLimit(limit)) => Attempt::Fatal(Unavailable::new(
                UnavailableKind::ResponseTooLarge,
                format!("{url} returned more than the {limit} byte limit"),
            )),
            Err(ureq::Error::Timeout(_)) => Attempt::Retry(Unavailable::new(
                UnavailableKind::Timeout,
                format!("reading {url} exceeded {:?}", self.options.timeout),
            )),
            Err(error) => Attempt::Retry(Unavailable::new(
                UnavailableKind::Transport,
                format!("reading {url} failed: {error}"),
            )),
        }
    }

    fn request(&self, url: &str, body: Option<&[u8]>) -> Result<Option<Vec<u8>>, Unavailable> {
        let attempts = self.options.max_retries.saturating_add(1);
        let mut last: Option<Unavailable> = None;
        for attempt in 0..attempts {
            if attempt > 0 {
                let factor = 1u32 << (attempt - 1).min(4);
                thread::sleep(self.options.retry_backoff.saturating_mul(factor));
            }
            match self.attempt(url, body) {
                Attempt::Body(bytes) => return Ok(Some(bytes)),
                Attempt::Missing => return Ok(None),
                Attempt::Fatal(reason) => return Err(reason),
                Attempt::Retry(reason) => last = Some(reason),
            }
        }
        // IMPORTANT: the last failure keeps its own kind. Relabelling a timeout or a 429 as
        // "retries exhausted" would hide from the caller what actually went wrong.
        Err(match last {
            Some(reason) if attempts > 1 => Unavailable::new(
                reason.kind(),
                format!("{attempts} attempts failed; last: {}", reason.detail()),
            ),
            Some(reason) => reason,
            None => Unavailable::new(
                UnavailableKind::Transport,
                format!("{url} was never attempted"),
            ),
        })
    }

    fn get(&self, path: &str) -> Lookup<Vec<u8>> {
        let url = format!("{}{path}", self.base_url);
        match self.request(&url, None) {
            Ok(Some(bytes)) => Lookup::Found(bytes),
            Ok(None) => Lookup::NotFound,
            Err(reason) => Lookup::Unavailable(reason),
        }
    }

    fn put_record(&self, record: &RegistryRecord) -> Result<(), RegistryError> {
        let Some(auth) = &self.auth else {
            return Err(RegistryError::RemotePublish {
                detail: "authenticated HTTP backend required for publication".to_owned(),
            });
        };
        let payload = record.to_envelope_bytes()?;
        let url = format!(
            "{}/v0/record/{}",
            self.base_url,
            record.record_id().to_hex()
        );
        let attempts = self.options.max_retries.saturating_add(1);
        let mut last = String::new();
        for attempt in 0..attempts {
            if attempt > 0 {
                let factor = 1u32 << (attempt - 1).min(4);
                thread::sleep(self.options.retry_backoff.saturating_mul(factor));
            }
            let request = self
                .agent
                .put(&url)
                .header("accept", "application/json")
                .header("content-type", "application/json");
            let response = match auth {
                HttpRegistryAuth::Bearer(token) => request
                    .header("authorization", &format!("Bearer {token}"))
                    .send(&payload),
                HttpRegistryAuth::CloudflareAccess {
                    client_id,
                    client_secret,
                } => request
                    .header("cf-access-client-id", client_id)
                    .header("cf-access-client-secret", client_secret)
                    .send(&payload),
            };
            match response {
                Ok(response) if matches!(response.status().as_u16(), 200 | 201 | 204) => {
                    return Ok(());
                }
                Ok(response) => {
                    let status = response.status().as_u16();
                    last = format!("{url} answered {status}");
                    if !matches!(status, 429 | 500..=599) {
                        break;
                    }
                }
                Err(ureq::Error::Timeout(_)) => {
                    last = format!("request to {url} exceeded {:?}", self.options.timeout);
                }
                Err(error) => last = format!("request to {url} failed: {error}"),
            }
        }
        Err(RegistryError::RemotePublish {
            detail: if attempts > 1 {
                format!("{attempts} attempts failed; last: {last}")
            } else {
                last
            },
        })
    }
}

fn malformed(detail: impl Into<String>) -> Unavailable {
    Unavailable::new(UnavailableKind::MalformedResponse, detail)
}

#[derive(Debug, Deserialize)]
struct MarkResponse {
    records: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct CandidateResponse {
    candidates: Vec<WireCandidate>,
}

#[derive(Debug, Deserialize)]
struct WireCandidate {
    record_id: RecordId,
    advisory_score: f32,
}

fn record_from_bytes(bytes: &[u8]) -> Result<RegistryRecord, Unavailable> {
    RegistryRecord::from_envelope_bytes(bytes)
        .map_err(|error| malformed(format!("record is not admissible: {error}")))
}

impl RegistryBackend for HttpRegistryBackend {
    fn source(&self) -> &RegistrySource {
        &self.source
    }

    fn lookup_by_mark(&self, mark: &MarkId) -> Lookup<MarkMatches> {
        let bytes = match self.get(&format!("/v0/mark/{}", mark.to_hex())) {
            Lookup::Found(bytes) => bytes,
            Lookup::NotFound => return Lookup::NotFound,
            Lookup::Unavailable(reason) => return Lookup::Unavailable(reason),
        };
        let response: MarkResponse = match serde_json::from_slice(&bytes) {
            Ok(response) => response,
            Err(error) => return Lookup::Unavailable(malformed(error.to_string())),
        };
        if response.records.is_empty() {
            return Lookup::NotFound;
        }
        if response.records.len() > MAX_MARK_MATCHES {
            return Lookup::Unavailable(malformed(format!(
                "{} records returned for one mark, over the {MAX_MARK_MATCHES} limit",
                response.records.len()
            )));
        }
        let mut records = Vec::with_capacity(response.records.len());
        for envelope in &response.records {
            let bytes = match serde_json::to_vec(envelope) {
                Ok(bytes) => bytes,
                Err(error) => return Lookup::Unavailable(malformed(error.to_string())),
            };
            let record = match record_from_bytes(&bytes) {
                Ok(record) => record,
                Err(reason) => return Lookup::Unavailable(reason),
            };
            // Every field of this mark id is re-derived from the manifest the server sent: the
            // locator from the document's own public key and locator_salt, the version and
            // namespace from its own `mark` block. A server cannot swap in a manifest that does
            // not claim this mark. 48 bits is still an index rather than an identity, which is why
            // the caller re-derives the full RecordId before the match reaches a verdict.
            if record.mark_id() != *mark {
                return Lookup::Unavailable(malformed(format!(
                    "record {} does not carry the requested mark {}",
                    record.mark_id(),
                    mark
                )));
            }
            records.push(record);
        }
        match MarkMatches::new(records) {
            Ok(matches) => Lookup::Found(matches),
            Err(error) => Lookup::Unavailable(malformed(error.to_string())),
        }
    }

    fn lookup_by_content_hash(&self, content_hash: &ContentHash) -> Lookup<RegistryRecord> {
        let bytes = match self.get(&format!("/v0/content/{}", content_hash.to_hex())) {
            Lookup::Found(bytes) => bytes,
            Lookup::NotFound => return Lookup::NotFound,
            Lookup::Unavailable(reason) => return Lookup::Unavailable(reason),
        };
        // IMPORTANT: unlike the mark and record-id guards, this compares server-supplied
        // metadata. Nothing in the manifest commits to a content hash, so this is a shape check
        // that catches a confused backend, not a defence against a hostile one. The hard binding
        // is what actually decides whether this manifest describes this audio.
        match record_from_bytes(&bytes) {
            Ok(record) if record.content_hash() == *content_hash => Lookup::Found(record),
            Ok(record) => Lookup::Unavailable(malformed(format!(
                "record {} answers for content hash {}, not {content_hash}",
                record.record_id(),
                record.content_hash()
            ))),
            Err(reason) => Lookup::Unavailable(reason),
        }
    }

    fn nearest_by_fingerprint(
        &self,
        query: &Fingerprint,
        limit: usize,
    ) -> Lookup<Vec<ScoredCandidate>> {
        let limit = limit.min(MAX_FINGERPRINT_CANDIDATES);
        if limit == 0 {
            return Lookup::NotFound;
        }
        let request = serde_json::json!({
            "fingerprint": query.to_hex(),
            "limit": limit,
        });
        let payload = match serde_json::to_vec(&request) {
            Ok(payload) => payload,
            Err(error) => return Lookup::Unavailable(malformed(error.to_string())),
        };
        let url = format!("{}/v0/fingerprint", self.base_url);
        let bytes = match self.request(&url, Some(&payload)) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Lookup::NotFound,
            Err(reason) => return Lookup::Unavailable(reason),
        };
        let response: CandidateResponse = match serde_json::from_slice(&bytes) {
            Ok(response) => response,
            Err(error) => return Lookup::Unavailable(malformed(error.to_string())),
        };
        if response.candidates.is_empty() {
            return Lookup::NotFound;
        }
        if response.candidates.len() > limit {
            return Lookup::Unavailable(malformed(format!(
                "{} candidates returned for a limit of {limit}",
                response.candidates.len()
            )));
        }
        let mut candidates = Vec::with_capacity(response.candidates.len());
        for candidate in response.candidates {
            match AdvisoryScore::new(candidate.advisory_score) {
                Ok(score) => candidates.push(ScoredCandidate::new(candidate.record_id, score)),
                Err(error) => return Lookup::Unavailable(malformed(error.to_string())),
            }
        }
        Lookup::Found(candidates)
    }

    fn fetch(&self, record_id: &RecordId) -> Lookup<RegistryRecord> {
        let bytes = match self.get(&format!("/v0/record/{}", record_id.to_hex())) {
            Lookup::Found(bytes) => bytes,
            Lookup::NotFound => return Lookup::NotFound,
            Lookup::Unavailable(reason) => return Lookup::Unavailable(reason),
        };
        match record_from_bytes(&bytes) {
            Ok(record) if record.record_id() == *record_id => Lookup::Found(record),
            Ok(record) => Lookup::Unavailable(malformed(format!(
                "record {} was returned for a request for {record_id}",
                record.record_id()
            ))),
            Err(reason) => Lookup::Unavailable(reason),
        }
    }
}

impl WritableRegistryBackend for HttpRegistryBackend {
    fn put(&self, record: &RegistryRecord) -> Result<(), RegistryError> {
        self.put_record(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_urls_that_could_smuggle_a_credential_or_a_path_are_refused() {
        for hostile in [
            "",
            "ftp://example.test",
            "https://user:pass@example.test",
            "https://example.test?token=abc",
            "https://example.test/../admin",
            "example.test",
            "https://example.test/ path",
        ] {
            assert!(
                validate_base_url(hostile).is_err(),
                "{hostile} was accepted"
            );
        }
        assert_eq!(
            validate_base_url("https://example.test/registry/").unwrap(),
            "https://example.test/registry"
        );
    }
}
