//! The options object JavaScript passes, and what it can legally ask for in a wasm build.

use std::sync::Arc;

use audio_provenance_sdk::VerifyOptions;
use apw_trace::{FileTrustStore, NullTestTable, trust::TRUST_STORE_FORMAT};
use audio_provenance_trust::{AnchoredTrustStore, Instant, TrustStore, store::MAX_STORE_BYTES};
use serde::Deserialize;
use wasm_bindgen::JsValue;

use crate::json::{coded, invalid_option};
use crate::records::CallerSuppliedRecords;

pub const DEFAULT_REGISTRY_NAME: &str = "caller";

/// IMPORTANT: `deny_unknown_fields`. A misspelled option that is silently ignored is a caller who
/// believes a trust store or a null-test report is in force when it is not, and both of those
/// change the verdict.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    /// Registry record envelopes, exactly as `audio-provenance registry` stores them.
    #[serde(default)]
    pub records: Vec<serde_json::Value>,
    /// The name the result reports as the answering registry.
    #[serde(default)]
    pub registry_name: Option<String>,
    /// A trust store document: the flat `audio-provenance-trust-store-v0`, or the chained
    /// `audio-provenance-trust-store-v1` whose signed revocation lists the verifier consults.
    /// Without one, no identity is ever disclosed.
    #[serde(default)]
    pub trust_store: Option<serde_json::Value>,
    /// RFC 3339 UTC instant the chained store's validity windows and revocations are judged
    /// against. Defaults to the JavaScript clock in a wasm build; a native build has no clock this
    /// crate may read, so it must be supplied.
    #[serde(default)]
    pub trust_evaluated_at: Option<String>,
    /// A `audio-provenance-bench` null-test report. Without one, no soft binding reaches `verified`.
    #[serde(default)]
    pub null_test: Option<serde_json::Value>,
    #[serde(default)]
    pub offline: bool,
    #[serde(default)]
    pub soft_binding_threshold: Option<f64>,
    #[serde(default)]
    pub accept_inferred_association: bool,
}

impl Options {
    pub fn parse(json: Option<&str>) -> Result<Self, JsValue> {
        match json {
            None => Ok(Self::default()),
            Some(text) => serde_json::from_str(text)
                .map_err(|error| invalid_option("options", &error.to_string())),
        }
    }

    pub fn build(self) -> Result<VerifyOptions, JsValue> {
        let mut options = VerifyOptions::new()
            .offline(self.offline)
            .accept_inferred_association(self.accept_inferred_association);

        if let Some(threshold) = self.soft_binding_threshold {
            options = options
                .with_soft_binding_threshold(threshold)
                .map_err(coded)?;
        }

        if !self.records.is_empty() {
            let name = self
                .registry_name
                .as_deref()
                .unwrap_or(DEFAULT_REGISTRY_NAME);
            let backend =
                CallerSuppliedRecords::from_envelopes(name, &self.records).map_err(coded)?;
            options = options.with_registry(Arc::new(backend));
        }

        if let Some(document) = self.trust_store {
            let bytes = serde_json::to_vec(&document)
                .map_err(|error| invalid_option("trustStore", &error.to_string()))?;
            let flat = document.get("format").and_then(serde_json::Value::as_str)
                == Some(TRUST_STORE_FORMAT);
            if flat {
                options = options
                    .with_trust_store(Box::new(FileTrustStore::from_json(&bytes).map_err(coded)?));
            } else {
                // The size bound lives at this boundary for the same reason it does in the CLI:
                // the loader cannot refuse bytes the caller already made resident.
                if bytes.len() as u64 > MAX_STORE_BYTES {
                    return Err(invalid_option(
                        "trustStore",
                        &format!("exceeds the {MAX_STORE_BYTES} byte limit"),
                    ));
                }
                let store = TrustStore::from_json(&bytes).map_err(coded)?;
                let at = evaluation_instant(self.trust_evaluated_at.as_deref())?;
                options = options.with_trust_store(Box::new(AnchoredTrustStore::new(store, at)));
            }
        }

        if let Some(report) = self.null_test {
            let bytes = serde_json::to_vec(&report)
                .map_err(|error| invalid_option("nullTest", &error.to_string()))?;
            options =
                options.with_null_test(NullTestTable::from_bench_report(&bytes).map_err(coded)?);
        }

        Ok(options)
    }
}

fn evaluation_instant(given: Option<&str>) -> Result<Instant, JsValue> {
    if let Some(text) = given {
        return Instant::parse(text).map_err(coded);
    }
    clock_now()
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn clock_now() -> Result<Instant, JsValue> {
    let millis = js_sys::Date::now();
    if !millis.is_finite() {
        return Err(invalid_option("trustEvaluatedAt", "the JavaScript clock is unreadable"));
    }
    Instant::from_unix_seconds((millis / 1000.0).floor() as i64).map_err(coded)
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn clock_now() -> Result<Instant, JsValue> {
    Err(invalid_option(
        "trustEvaluatedAt",
        "a chained trust store needs an evaluation instant and this build has no clock",
    ))
}
