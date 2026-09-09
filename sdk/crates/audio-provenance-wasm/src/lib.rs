//! The wasm-bindgen surface over [`audio_provenance_sdk`]: verify and inspect, from a browser or from Node.
//!
//! # What this build can do, and what it cannot
//!
//! `wasm32-unknown-unknown` has no filesystem and no socket. Every capability that depends on one
//! is ABSENT here rather than stubbed, and [`capabilities`] says so in a `platform` block instead of
//! reprinting the native answer:
//!
//! | Capability | Here |
//! |---|---|
//! | Decode wav / aiff / mp3 / flac / ogg / mp4 from bytes | yes |
//! | Rung 1, embedded manifest | yes |
//! | Rung 2, sidecar manifest | no, there is no directory to resolve one against |
//! | Rungs 3-6 (content hash, decoded-audio hash, Watermark, fingerprint search) | only over the records the caller supplies |
//! | `local` filesystem registry | no |
//! | `http` registry | no; a browser cannot open a raw socket, and the JS side fetches instead |
//! | Trust store, null-test report | yes, passed in as JSON rather than read from a path |
//! | Signing, marking, publishing | no; the producer surface is native-only |
//!
//! Nothing about the verdict changes. The status mapping, the soft-binding gates, the null-test
//! requirement and the identity rule are `apw_trace`'s, compiled unmodified.
//!
//! # Two calls, not a fake synchronous fetch
//!
//! `RegistryBackend` is synchronous, and `fetch` in JavaScript is not. Rather than pretend
//! otherwise, a caller who needs a remote registry runs [`locators`] first, fetches the records for
//! the locators it returns, and passes them to [`verify_bytes`] in `options.records`. A file carrying an
//! embedded manifest needs no registry at all and verifies in one call.

use audio_provenance_core::VerificationStatus;
use apw_watermark::Watermark;
use apw_trace::{IngestLimits, ingest_bytes};
use serde::Serialize;
use wasm_bindgen::prelude::*;

mod json;
mod options;
mod records;

use crate::json::{coded, to_camel_json};
use crate::options::Options;

pub use crate::records::CallerSuppliedRecords;

/// Turns a wasm trap into a readable JS stack trace. Without it a panic anywhere in the graph
/// reaches the caller as `RuntimeError: unreachable` and nothing else.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

/// Verifies audio already in memory.
///
/// `options_json` is the JSON text of the options object; `null` or omitted means defaults. The
/// return is the JSON text of a `VerifyResult` with every key in camelCase.
#[wasm_bindgen(js_name = verifyBytes)]
pub fn verify_bytes(audio: Vec<u8>, options_json: Option<String>) -> Result<String, JsValue> {
    let options = Options::parse(options_json.as_deref())?.build()?;
    let result = audio_provenance_sdk::verify_bytes(audio, &options).map_err(coded)?;
    to_camel_json(&result)
}

/// Reports what the recovery ladder found, with no verdict.
///
/// The native `inspect` takes a path so it can run the sidecar rung; there is no directory here, so
/// this reaches the ladder through `verify_bytes` and drops the verdict on the way out. One
/// consequence is inherited and worth stating: a search that could not finish RAISES here, where
/// the native `inspect` would have reported it. With caller-supplied records that needs a supplied
/// record set to be internally inconsistent, which is a caller error either way.
#[wasm_bindgen(js_name = inspectBytes)]
pub fn inspect_bytes(audio: Vec<u8>, options_json: Option<String>) -> Result<String, JsValue> {
    let options = Options::parse(options_json.as_deref())?.build()?;
    let ingested = ingest_bytes(audio, IngestLimits::default()).map_err(coded)?;
    let container = ingested.container().as_str();
    let buffer = ingested.audio();
    let sample_rate = buffer.sample_rate();
    let channels = buffer.channels();
    let duration_seconds = buffer.duration_seconds();
    // PERF: decodes twice, once here for the container facts and once inside the ladder. `inspect`
    // is a diagnostic call and the alternative is a second entry point into Trace that borrows
    // an already-ingested buffer, which is a wider change than this report is worth.
    let bytes = ingested.bytes().to_vec();
    drop(ingested);
    let result = audio_provenance_sdk::verify_bytes(bytes, &options).map_err(coded)?;
    to_camel_json(&InspectBytesReport {
        container,
        content_sha256: result.content_sha256.clone(),
        content_bytes: result.content_bytes,
        decoded_audio_sha256: result.decoded_audio_sha256.clone(),
        sample_rate,
        channels,
        duration_seconds,
        manifest_recovered: result.method.is_some(),
        manifest_schema: result
            .manifest
            .as_ref()
            .map(|manifest| manifest.schema().id()),
        method: result.method.map(apw_trace::RecoveryMethod::as_str),
        signer_id: result
            .signature
            .as_ref()
            .and_then(|signature| signature.signer_id.clone()),
        trace: result.trace,
        recovery: result.recovery,
        incomplete: result.incomplete,
    })
}

/// What this build can actually do, with the wasm platform limits stated rather than implied.
#[wasm_bindgen(js_name = capabilities)]
pub fn capabilities() -> Result<String, JsValue> {
    let mut tree = serde_json::to_value(audio_provenance_sdk::capabilities())
        .map_err(|error| json::throw("serialization_failed", &error.to_string()))?;
    let serde_json::Value::Object(fields) = &mut tree else {
        return Err(json::throw(
            "serialization_failed",
            "capabilities did not serialise to an object",
        ));
    };
    // IMPORTANT: the native answer says `registry_backends: ["local", "http"]`. That is false in a
    // build with no filesystem and no socket, and reprinting it would put a confidently wrong
    // sentence in front of every browser caller. One source of truth, one honest override.
    fields.insert(
        "registry_backends".to_string(),
        serde_json::json!(["caller_supplied_records"]),
    );
    fields.insert(
        "platform".to_string(),
        serde_json::to_value(PLATFORM)
            .map_err(|error| json::throw("serialization_failed", &error.to_string()))?,
    );
    json::camelize(&mut tree);
    serde_json::to_string(&tree)
        .map_err(|error| json::throw("serialization_failed", &error.to_string()))
}

/// The Watermark locators recoverable from this audio, for a caller that must fetch records before
/// it can supply them.
///
/// At most one: blind detection reports the single payload the CRC accepted, and `locators` is
/// empty when nothing decoded. A locator is an INDEX, not an identity: several records can share
/// one, and recovering one proves nothing on its own. Only [`verify_bytes`] produces a verdict.
#[wasm_bindgen(js_name = locators)]
pub fn locators(audio: Vec<u8>) -> Result<String, JsValue> {
    let ingested = ingest_bytes(audio, IngestLimits::default()).map_err(coded)?;
    let outcome = Watermark::public().detect(ingested.audio()).map_err(coded)?;
    let found = outcome.payload().map(|payload| LocatorReport {
        version: payload.version(),
        namespace: payload.namespace(),
        locator_hex: hex::encode(payload.locator_bytes()),
        confidence_class: outcome.class().as_str(),
        blocks_accepted: outcome.blocks_accepted(),
    });
    to_camel_json(&LocatorsReport {
        content_sha256: ingested.content_sha256().to_string(),
        locators: found.into_iter().collect(),
    })
}

/// The four statuses, so a JS test can assert the union has not silently grown.
#[wasm_bindgen(js_name = statuses)]
pub fn statuses() -> Vec<String> {
    [
        VerificationStatus::Verified,
        VerificationStatus::Changed,
        VerificationStatus::Untrusted,
        VerificationStatus::NotFound,
    ]
    .iter()
    .map(|status| status.as_str().to_string())
    .collect()
}

#[wasm_bindgen(js_name = version)]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

const PLATFORM: Platform = Platform {
    target: "wasm32-unknown-unknown",
    filesystem: false,
    network: false,
    sidecar_manifest: false,
    registry_backends: ["caller_supplied_records"],
    signing: false,
    marking: false,
    note: "Rungs beyond the embedded manifest run only over records the caller supplies; a browser cannot open a socket, so fetch them in JavaScript and pass them to verify().",
};

#[derive(Debug, Clone, Copy, Serialize)]
struct Platform {
    target: &'static str,
    filesystem: bool,
    network: bool,
    sidecar_manifest: bool,
    registry_backends: [&'static str; 1],
    signing: bool,
    marking: bool,
    note: &'static str,
}

#[derive(Debug, Serialize)]
struct InspectBytesReport {
    container: &'static str,
    content_sha256: String,
    content_bytes: u64,
    decoded_audio_sha256: Option<String>,
    sample_rate: u32,
    channels: usize,
    duration_seconds: f64,
    manifest_recovered: bool,
    manifest_schema: Option<&'static str>,
    method: Option<&'static str>,
    signer_id: Option<String>,
    trace: Vec<apw_trace::RecoveryStep>,
    recovery: apw_trace::RecoveryReport,
    incomplete: bool,
}

#[derive(Debug, Serialize)]
struct LocatorsReport {
    content_sha256: String,
    locators: Vec<LocatorReport>,
}

#[derive(Debug, Serialize)]
struct LocatorReport {
    version: u8,
    namespace: u8,
    locator_hex: String,
    confidence_class: &'static str,
    blocks_accepted: usize,
}
