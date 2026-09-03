//! The options object JavaScript passes, and what it can legally ask for in a wasm build.

use std::sync::Arc;

use audio_provenance_sdk::VerifyOptions;
use genotrace::{FileTrustStore, NullTestTable};
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
    /// A `audio-provenance-trust-store-v0` document. Without one, no identity is ever disclosed.
    #[serde(default)]
    pub trust_store: Option<serde_json::Value>,
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
            options = options
                .with_trust_store(Box::new(FileTrustStore::from_json(&bytes).map_err(coded)?));
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
