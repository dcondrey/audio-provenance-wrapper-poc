use std::path::Path;

use serde_json::{json, Map, Number, Value};

use crate::canonical::{
    canonical_json_ascii, canonical_json_utf8, pretty_json_bytes, without_top_level_keys,
    LOCAL_SIGNATURE_EXCLUDED_KEYS, PORTABLE_SIGNATURE_EXCLUDED_KEYS,
};
use crate::error::{CoreError, Result};
use crate::proof::{ProofLevel, PROOF_LEVEL_KEY};
use crate::pyvalue::{get, is_string_equal, is_truthy, python_repr, python_str};
use crate::timestamp::utc_timestamp;

pub const APW_VERSION: &str = "0.9.0";
pub const MANIFEST_SCHEMA: &str = "audio-provenance-manifest-v0";
pub const CORE_PRINCIPLE: &str = "Never claim full DAW provenance.";

pub const C2PA_CLAIM_SCOPE: &str =
    "A real C2PA claim signed with a locally issued X.509 chain and bound to the \
     asset by a SHA-256 hard binding. The signing act and the binding are directly \
     observed. The signer identity is self-asserted: a 'verified' validation state \
     means the claim chains to this machine's own root certificate, not to any \
     external trust list, registry, or verified creator identity.";

pub const DEFAULT_UNOBSERVED: [&str; 5] = [
    "hidden_plugin_state",
    "internal_preset_logic",
    "bypassed_routing",
    "daw_internal_processing",
    "unverifiable_upstream_provenance",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    CompleteObservedPath,
    PartialObservedPath,
    UnknownCoverage,
}

impl CoverageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CoverageStatus::CompleteObservedPath => "complete_observed_path",
            CoverageStatus::PartialObservedPath => "partial_observed_path",
            CoverageStatus::UnknownCoverage => "unknown_coverage",
        }
    }

    pub fn parse(value: &str) -> Result<CoverageStatus> {
        match value {
            "complete_observed_path" => Ok(CoverageStatus::CompleteObservedPath),
            "partial_observed_path" => Ok(CoverageStatus::PartialObservedPath),
            "unknown_coverage" => Ok(CoverageStatus::UnknownCoverage),
            other => Err(CoreError::InvalidEnumValue {
                field: "coverage_status",
                value: other.to_owned(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssociationStatus {
    InferredMatch,
    NotEstablished,
    Unavailable,
}

impl AssociationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            AssociationStatus::InferredMatch => "inferred_match",
            AssociationStatus::NotEstablished => "not_established",
            AssociationStatus::Unavailable => "unavailable",
        }
    }

    pub fn parse(value: &str) -> Result<AssociationStatus> {
        match value {
            "inferred_match" => Ok(AssociationStatus::InferredMatch),
            "not_established" => Ok(AssociationStatus::NotEstablished),
            "unavailable" => Ok(AssociationStatus::Unavailable),
            other => Err(CoreError::InvalidEnumValue {
                field: "association_status",
                value: other.to_owned(),
            }),
        }
    }

    /// INVARIANT (schema-enforced): `InferredMatch` pairs only with `Inferred`;
    /// `NotEstablished` and `Unavailable` pair only with `UnknownUnobserved`.
    /// A routed-feature match is an inference, never a direct observation of
    /// the export's own bytes.
    pub fn required_proof_level(self) -> ProofLevel {
        match self {
            AssociationStatus::InferredMatch => ProofLevel::Inferred,
            AssociationStatus::NotEstablished | AssociationStatus::Unavailable => {
                ProofLevel::UnknownUnobserved
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Issued,
    Degraded,
    Unknown,
}

impl ReceiptStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ReceiptStatus::Issued => "issued",
            ReceiptStatus::Degraded => "degraded",
            ReceiptStatus::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> Result<ReceiptStatus> {
        match value {
            "issued" => Ok(ReceiptStatus::Issued),
            "degraded" => Ok(ReceiptStatus::Degraded),
            "unknown" => Ok(ReceiptStatus::Unknown),
            other => Err(CoreError::InvalidEnumValue {
                field: "receipt_status",
                value: other.to_owned(),
            }),
        }
    }
}

/// `c2pa_mapping` is a descriptive projection; `c2pa_claim` is the signed claim.
/// They are not interchangeable and neither substitutes for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum C2paClaimStatus {
    Embedded,
    Sidecar,
    Unavailable,
}

impl C2paClaimStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            C2paClaimStatus::Embedded => "embedded",
            C2paClaimStatus::Sidecar => "sidecar",
            C2paClaimStatus::Unavailable => "unavailable",
        }
    }

    pub fn parse(value: &str) -> Result<C2paClaimStatus> {
        match value {
            "embedded" => Ok(C2paClaimStatus::Embedded),
            "sidecar" => Ok(C2paClaimStatus::Sidecar),
            "unavailable" => Ok(C2paClaimStatus::Unavailable),
            other => Err(CoreError::InvalidEnumValue {
                field: "c2pa_claim_status",
                value: other.to_owned(),
            }),
        }
    }

    pub fn is_signed(self) -> bool {
        matches!(self, C2paClaimStatus::Embedded | C2paClaimStatus::Sidecar)
    }

    /// The signing act is observed. IMPORTANT: this is the proof level of the
    /// act, not of the signer's identity, which stays `not_established`.
    pub fn claim_proof_level(self) -> ProofLevel {
        if self.is_signed() {
            ProofLevel::DirectlyObserved
        } else {
            ProofLevel::UnknownUnobserved
        }
    }
}

pub fn unavailable_c2pa_claim(reason: &str) -> Value {
    json!({
        "status": "unavailable",
        "reason": reason,
        "scope": C2PA_CLAIM_SCOPE,
        PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StemEvidence {
    pub stem_id: String,
    pub hash_chain_root: String,
    pub hash_chain_genesis: String,
    pub hash_chain_length: u64,
    pub first_observed_ms: i64,
    pub last_observed_ms: i64,
    pub first_received_at: Option<String>,
    pub last_received_at: Option<String>,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub source_category: String,
    pub source_category_proof_level: ProofLevel,
    pub plugin_instance_ids: Vec<String>,
    pub proof_level: ProofLevel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportEvidence {
    pub file_path: String,
    pub file_name: String,
    pub sha256: String,
    pub format: String,
    pub file_size_bytes: u64,
    pub duration_seconds: Option<f64>,
    /// REQUIRED: `Number`, not `f64`. Python's `extract_audio_metadata` returns
    /// an `int` from the wave/aifc path and an `int`-or-`float` from the afinfo
    /// path, so `44100` and `44100.0` are different canonical bytes.
    pub sample_rate_hz: Option<Number>,
    pub channel_count: Option<u16>,
    pub exported_at: String,
    pub export_version: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IngredientEvidence {
    pub file_name: String,
    pub sha256: String,
    pub proof_level: ProofLevel,
    pub correlation_confidence: Option<f64>,
    pub audio_fingerprint: Option<Value>,
}

/// Insertion-ordered manifest document. Order is cosmetic (the pretty file);
/// both signing inputs sort keys, so a reordering never breaks a signature.
///
/// INVARIANT: the inner `Value` is always `Value::Object`; [`Manifest::from_value`]
/// is the only constructor and rejects anything else.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest(Value);

impl Manifest {
    pub fn from_value(value: Value) -> Result<Self> {
        if value.is_object() {
            Ok(Manifest(value))
        } else {
            Err(CoreError::NotAnObject)
        }
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }

    pub fn as_object(&self) -> &Map<String, Value> {
        static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
        match &self.0 {
            Value::Object(map) => map,
            _ => EMPTY.get_or_init(Map::new),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0.get_mut(key)
    }

    /// Python dict semantics: a new key appends, an existing key keeps its slot.
    pub fn insert(&mut self, key: &str, value: Value) -> Option<Value> {
        match &mut self.0 {
            Value::Object(map) => map.insert(key.to_owned(), value),
            _ => None,
        }
    }

    pub fn session_id(&self) -> Option<&str> {
        self.0.get("session_id").and_then(Value::as_str)
    }

    /// `canonical_json_utf8` over the document minus
    /// [`PORTABLE_SIGNATURE_EXCLUDED_KEYS`].
    pub fn portable_signing_input(&self) -> Result<Vec<u8>> {
        canonical_json_utf8(&without_top_level_keys(
            &self.0,
            &PORTABLE_SIGNATURE_EXCLUDED_KEYS,
        ))
    }

    /// `canonical_json_ascii` over the document minus
    /// [`LOCAL_SIGNATURE_EXCLUDED_KEYS`].
    ///
    /// IMPORTANT: this input includes `portable_signature`, so the portable
    /// signature MUST be attached before the local signature is computed.
    pub fn local_signing_input(&self) -> Result<Vec<u8>> {
        canonical_json_ascii(&without_top_level_keys(
            &self.0,
            &LOCAL_SIGNATURE_EXCLUDED_KEYS,
        ))
    }

    pub fn to_pretty_bytes(&self) -> Result<Vec<u8>> {
        pretty_json_bytes(&self.0)
    }

    pub fn write_pretty(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|source| CoreError::io(parent, source))?;
            }
        }
        let bytes = self.to_pretty_bytes()?;
        std::fs::write(path, bytes).map_err(|source| CoreError::io(path, source))
    }
}

#[derive(Debug, Clone)]
pub struct ManifestBuilder {
    session_id: String,
    created_at: String,
    stems: Vec<StemEvidence>,
    export: Option<ExportEvidence>,
    ingredients: Vec<IngredientEvidence>,
    composite_edits: Vec<Value>,
    hardware_binding: Option<Value>,
    forgery_report: Option<Value>,
    coverage: Option<Value>,
    c2pa_claim: Option<Value>,
    audio_association: Option<Value>,
    session_diagnostics: Option<Value>,
    host_environment: Option<Value>,
    unobserved: Vec<String>,
}

impl Default for ManifestBuilder {
    fn default() -> Self {
        ManifestBuilder {
            session_id: String::new(),
            created_at: utc_timestamp(None),
            stems: Vec::new(),
            export: None,
            ingredients: Vec::new(),
            composite_edits: Vec::new(),
            hardware_binding: None,
            forgery_report: None,
            coverage: None,
            c2pa_claim: None,
            audio_association: None,
            session_diagnostics: None,
            host_environment: None,
            unobserved: DEFAULT_UNOBSERVED.iter().map(|s| (*s).to_owned()).collect(),
        }
    }
}

impl ManifestBuilder {
    pub fn new(session_id: impl Into<String>, created_at: String) -> Self {
        ManifestBuilder {
            session_id: session_id.into(),
            created_at,
            ..ManifestBuilder::default()
        }
    }

    pub fn add_stem(&mut self, stem: StemEvidence) -> &mut Self {
        self.stems.push(stem);
        self
    }

    pub fn set_export(&mut self, export: ExportEvidence) -> &mut Self {
        self.export = Some(export);
        self
    }

    pub fn add_ingredient(&mut self, ingredient: IngredientEvidence) -> &mut Self {
        self.ingredients.push(ingredient);
        self
    }

    pub fn add_composite_edit(&mut self, edit: Value) -> &mut Self {
        self.composite_edits.push(edit);
        self
    }

    pub fn set_coverage(&mut self, coverage: Value) -> &mut Self {
        self.coverage = Some(coverage);
        self
    }

    pub fn set_audio_association(&mut self, association: Value) -> &mut Self {
        self.audio_association = Some(association);
        self
    }

    pub fn set_forgery_report(&mut self, report: Value) -> &mut Self {
        self.forgery_report = Some(report);
        self
    }

    pub fn set_session_diagnostics(&mut self, diagnostics: Value) -> &mut Self {
        self.session_diagnostics = Some(diagnostics);
        self
    }

    pub fn set_host_environment(&mut self, host_environment: Value) -> &mut Self {
        self.host_environment = Some(host_environment);
        self
    }

    pub fn set_hardware_binding(&mut self, binding: Value) -> &mut Self {
        self.hardware_binding = Some(binding);
        self
    }

    /// The real signed C2PA claim record produced by `apw-c2pa`. Absent,
    /// [`ManifestBuilder::build`] emits
    /// `unavailable_c2pa_claim("No C2PA claim was supplied to the builder.")`.
    pub fn set_c2pa_claim(&mut self, claim: Value) -> &mut Self {
        self.c2pa_claim = Some(claim);
        self
    }

    pub fn unobserved_mut(&mut self) -> &mut Vec<String> {
        &mut self.unobserved
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn build(&self) -> Result<Manifest> {
        let mut manifest = Map::new();
        manifest.insert("apw_version".to_owned(), json!(APW_VERSION));
        manifest.insert("schema".to_owned(), json!(MANIFEST_SCHEMA));
        manifest.insert("session_id".to_owned(), json!(self.session_id));
        manifest.insert(
            "capture_session".to_owned(),
            json!({
                "id": self.session_id,
                "scope": "local_daemon_runtime",
                PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
            }),
        );
        manifest.insert("created_at".to_owned(), json!(self.created_at));
        manifest.insert("core_principle".to_owned(), json!(CORE_PRINCIPLE));
        manifest.insert(
            "daemon_receipt_acknowledgement".to_owned(),
            json!({
                "protocol": "apw-local-udp-ack-v1",
                "status": ReceiptStatus::Unknown.as_str(),
                "streams": [],
                "counters": {"attempted": 0, "sent": 0, "failed": 0},
                "scope": "No daemon acknowledgement evidence was supplied to the builder.",
                PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
            }),
        );

        manifest.insert(
            "observed_stems".to_owned(),
            Value::Array(self.stems.iter().map(stem_json).collect()),
        );

        if let Some(export) = &self.export {
            manifest.insert("export".to_owned(), export_json(export));
        }

        if !self.ingredients.is_empty() {
            manifest.insert(
                "ingredients".to_owned(),
                Value::Array(self.ingredients.iter().map(ingredient_json).collect()),
            );
        }

        if !self.composite_edits.is_empty() {
            manifest.insert(
                "edit_history".to_owned(),
                Value::Array(self.composite_edits.clone()),
            );
        }

        if let Some(binding) = &self.hardware_binding {
            manifest.insert("hardware_binding".to_owned(), binding.clone());
        }

        if let Some(report) = &self.forgery_report {
            manifest.insert("forgery_analysis".to_owned(), report.clone());
        }

        manifest.insert(
            "apw:unobserved".to_owned(),
            Value::Array(self.unobserved.iter().map(|item| json!(item)).collect()),
        );

        manifest.insert("observation_coverage".to_owned(), self.coverage_or_default());

        if let Some(diagnostics) = &self.session_diagnostics {
            manifest.insert("session_diagnostics".to_owned(), diagnostics.clone());
        }

        if let Some(host_environment) = &self.host_environment {
            manifest.insert("host_environment".to_owned(), host_environment.clone());
        }

        manifest.insert(
            "stem_export_association".to_owned(),
            self.association_or_default(),
        );

        manifest.insert(
            "claim_summary".to_owned(),
            Value::Array(self.build_claim_summary()),
        );

        manifest.insert(
            "c2pa_mapping".to_owned(),
            json!({
                "status": "descriptive_projection_see_c2pa_claim_for_the_signed_claim",
                "claim_generator": format!("AudioProvenanceCapture/{APW_VERSION}"),
                "assertions": Value::Array(self.build_c2pa_assertions()),
            }),
        );

        let claim = match &self.c2pa_claim {
            Some(claim) if is_truthy(claim) => claim.clone(),
            _ => unavailable_c2pa_claim("No C2PA claim was supplied to the builder."),
        };
        manifest.insert("c2pa_claim".to_owned(), claim);

        Manifest::from_value(Value::Object(manifest))
    }

    fn coverage_or_default(&self) -> Value {
        match &self.coverage {
            Some(coverage) if is_truthy(coverage) => coverage.clone(),
            _ => json!({
                "status": CoverageStatus::UnknownCoverage.as_str(),
                "basis": "No complete observation counters were available.",
                "counters": {},
                PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
            }),
        }
    }

    fn association_or_default(&self) -> Value {
        match &self.audio_association {
            Some(association) if is_truthy(association) => association.clone(),
            _ => json!({
                "status": AssociationStatus::NotEstablished.as_str(),
                "capture_session_id": self.session_id,
                "stem_ids": self.stems.iter().map(|stem| json!(stem.stem_id)).collect::<Vec<Value>>(),
                "export_file_name": match &self.export {
                    Some(export) => json!(export.file_name),
                    None => Value::Null,
                },
                "basis": "No routed-feature/export comparison was supplied. Session co-occurrence \
                          alone does not establish an audio association.",
                "reason": "routed-feature comparison unavailable",
                PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
            }),
        }
    }

    /// A concise, proof-labelled fight card for human review.
    fn build_claim_summary(&self) -> Vec<Value> {
        let has_stem = !self.stems.is_empty();
        let has_export = self.export.is_some();
        let empty = Value::Object(Map::new());
        let association_record = match &self.audio_association {
            Some(association) if is_truthy(association) => association.clone(),
            _ => empty.clone(),
        };
        let association_map = association_record.as_object();
        let association = association_map
            .map(|map| matches!(get(map, "status"), Value::String(s) if s == "inferred_match"))
            .unwrap_or(false);
        let coverage_record = self.coverage_or_default_for_summary();
        let coverage_map = coverage_record.as_object();
        let host_record = match &self.host_environment {
            Some(host) if is_truthy(host) => host.clone(),
            _ => empty.clone(),
        };
        let host_map = host_record.as_object();
        let host_name = match host_map.and_then(|map| map.get("host_name")) {
            Some(name) if is_truthy(name) => name.clone(),
            _ => json!("unknown"),
        };

        let claim_record = match &self.c2pa_claim {
            Some(claim) if is_truthy(claim) => claim.clone(),
            _ => empty.clone(),
        };
        let claim_map = claim_record.as_object();
        let c2pa_status = claim_map
            .map(|map| match map.get("status") {
                Some(value) => python_str(value),
                None => "unavailable".to_owned(),
            })
            .unwrap_or_else(|| "unavailable".to_owned());
        let c2pa_signed = c2pa_status == "embedded" || c2pa_status == "sidecar";
        let validation = claim_map.and_then(|map| map.get("validation"));
        let c2pa_evidence = match (c2pa_signed, validation) {
            (true, Some(Value::Object(validation))) => format!(
                "{c2pa_status} claim, validation state {}; \
                 trust evaluated against this machine's own root only",
                python_str(get(validation, "state"))
            ),
            _ => claim_map
                .map(|map| match map.get("reason") {
                    Some(value) => python_str(value),
                    None => "No C2PA claim was produced".to_owned(),
                })
                .unwrap_or_else(|| "No C2PA claim was produced".to_owned()),
        };

        let windows: u64 = self
            .stems
            .iter()
            .fold(0u64, |total, stem| total.saturating_add(stem.hash_chain_length));

        vec![
            json!({
                "claim": "observation_coverage",
                "value": lookup_or(coverage_map, "status", json!(CoverageStatus::UnknownCoverage.as_str())),
                "evidence": lookup_or(coverage_map, "basis", json!("Complete counters were not available")),
                PROOF_LEVEL_KEY: lookup_or(coverage_map, PROOF_LEVEL_KEY, json!(ProofLevel::UnknownUnobserved.as_str())),
            }),
            json!({
                "claim": "routed_audio_observed",
                "value": has_stem,
                "evidence": if has_stem {
                    format!("{windows} hash windows received")
                } else {
                    "No buffer_hash events were received".to_owned()
                },
                PROOF_LEVEL_KEY: proof_str(has_stem),
            }),
            json!({
                "claim": "export_file_hashed",
                "value": has_export,
                "evidence": match &self.export {
                    Some(export) => export.sha256.clone(),
                    None => "No export detected".to_owned(),
                },
                PROOF_LEVEL_KEY: proof_str(has_export),
            }),
            json!({
                "claim": "observed_stem_linked_to_export",
                "value": association,
                "evidence": association_evidence(association_map),
                PROOF_LEVEL_KEY: if association {
                    ProofLevel::Inferred.as_str()
                } else {
                    ProofLevel::UnknownUnobserved.as_str()
                },
            }),
            json!({
                "claim": "source_category",
                "value": match self.stems.first() {
                    Some(stem) => stem.source_category.clone(),
                    None => "unknown".to_owned(),
                },
                "evidence": match self.stems.first() {
                    Some(_) => "producer declaration",
                    None => "no observed stem",
                },
                PROOF_LEVEL_KEY: match self.stems.first() {
                    Some(stem) => stem.source_category_proof_level.as_str(),
                    None => ProofLevel::UnknownUnobserved.as_str(),
                },
            }),
            json!({
                "claim": "host_application",
                "value": host_name,
                "evidence": lookup_or(
                    host_map,
                    "basis",
                    json!("No host environment was supplied to the builder."),
                ),
                PROOF_LEVEL_KEY: lookup_or(
                    host_map,
                    PROOF_LEVEL_KEY,
                    json!(ProofLevel::UnknownUnobserved.as_str()),
                ),
            }),
            json!({
                "claim": "full_ableton_provenance",
                "value": false,
                "evidence": "Only audio routed through the capture plugin was observed",
                PROOF_LEVEL_KEY: ProofLevel::UnknownUnobserved.as_str(),
            }),
            json!({
                "claim": "embedded_c2pa_claim",
                "value": c2pa_status,
                "evidence": c2pa_evidence,
                PROOF_LEVEL_KEY: proof_str(c2pa_signed),
            }),
        ]
    }

    /// IMPORTANT: the claim summary reads `self.coverage or {}`, not the
    /// defaulted `observation_coverage` block, so an absent coverage record
    /// falls through to the summary's own literal defaults.
    fn coverage_or_default_for_summary(&self) -> Value {
        match &self.coverage {
            Some(coverage) if is_truthy(coverage) => coverage.clone(),
            _ => Value::Object(Map::new()),
        }
    }

    fn build_c2pa_assertions(&self) -> Vec<Value> {
        let mut assertions: Vec<Value> = Vec::new();

        if let Some(export) = &self.export {
            assertions.push(json!({
                "label": "c2pa.hash.data",
                "data": {
                    "name": export.file_name,
                    "hash": export.sha256,
                    "algorithm": "sha256",
                },
            }));
        }

        for stem in &self.stems {
            assertions.push(json!({
                "label": "c2pa.ingredient",
                "data": {
                    "title": stem.stem_id,
                    "relationship": "componentOf",
                    "apw:hash_chain_root": stem.hash_chain_root,
                    PROOF_LEVEL_KEY: stem.proof_level.as_str(),
                },
            }));
        }

        for ingredient in &self.ingredients {
            assertions.push(json!({
                "label": "c2pa.ingredient",
                "data": {
                    "title": ingredient.file_name,
                    "relationship": "inputTo",
                    "hash": ingredient.sha256,
                    PROOF_LEVEL_KEY: ingredient.proof_level.as_str(),
                },
            }));
        }

        if !self.composite_edits.is_empty() {
            let actions: Vec<Value> = self
                .composite_edits
                .iter()
                .map(|edit| {
                    let edit_type = edit.get("edit_type").map(python_str);
                    let label = edit_type.clone().unwrap_or_else(|| "unknown".to_owned());
                    json!({
                        "action": c2pa_action_type(&label),
                        "when": edit.get("timestamp_ms").cloned().unwrap_or(Value::Null),
                        "apw:edit_type": edit.get("edit_type").cloned().unwrap_or(Value::Null),
                        "apw:confidence": edit.get("confidence").cloned().unwrap_or(Value::Null),
                        PROOF_LEVEL_KEY: ProofLevel::Inferred.as_str(),
                    })
                })
                .collect();
            assertions.push(json!({
                "label": "c2pa.actions",
                "data": {"actions": actions},
            }));
        }

        assertions.push(json!({
            "label": "apw.unobserved",
            "data": {"items": self.unobserved},
        }));

        assertions
    }
}

fn proof_str(observed: bool) -> &'static str {
    if observed {
        ProofLevel::DirectlyObserved.as_str()
    } else {
        ProofLevel::UnknownUnobserved.as_str()
    }
}

/// Render a measurement, or say it was not measured. Never `str(None)`.
fn measurement(value: &Value) -> String {
    match value {
        Value::Number(number) if number.is_f64() => match number.as_f64() {
            Some(float) => format!("{float:.4}"),
            None => "not measured".to_owned(),
        },
        Value::Number(_) => python_repr(value),
        _ => "not measured".to_owned(),
    }
}

/// Evidence line for the stem-to-export claim card.
///
/// IMPORTANT: this is signed and rendered verbatim on the fight card the founder
/// demo closes on, so an unavailable comparison reports its cause rather than
/// interpolating a missing value into the record.
fn association_evidence(record: Option<&Map<String, Value>>) -> String {
    let record = match record {
        Some(map) if !map.is_empty() => map,
        _ => return "No routed-feature comparison was supplied.".to_owned(),
    };
    let method = match get(record, "method") {
        value if is_truthy(value) => python_str(value),
        _ => "no method recorded".to_owned(),
    };
    if is_string_equal(get(record, "status"), "unavailable") {
        let reason = match record.get("reason") {
            Some(value) => python_str(value),
            None => "no reason recorded".to_owned(),
        };
        return format!("{method}: not measured ({reason})");
    }
    format!(
        "{method}: confidence {} / coverage {}",
        measurement(get(record, "confidence")),
        measurement(get(record, "matched_coverage")),
    )
}

fn lookup_or(map: Option<&Map<String, Value>>, key: &str, fallback: Value) -> Value {
    map.and_then(|map| map.get(key))
        .cloned()
        .unwrap_or(fallback)
}

fn stem_json(stem: &StemEvidence) -> Value {
    json!({
        "stem_id": stem.stem_id,
        "hash_chain_root": stem.hash_chain_root,
        "hash_chain_genesis": stem.hash_chain_genesis,
        "hash_chain_length": stem.hash_chain_length,
        "first_observed_ms": stem.first_observed_ms,
        "last_observed_ms": stem.last_observed_ms,
        "first_received_at": stem.first_received_at,
        "last_received_at": stem.last_received_at,
        "sample_rate_hz": stem.sample_rate_hz,
        "channel_count": stem.channel_count,
        "source_category": stem.source_category,
        "source_category_proof_level": stem.source_category_proof_level.as_str(),
        "source": {
            "category": stem.source_category,
            PROOF_LEVEL_KEY: stem.source_category_proof_level.as_str(),
        },
        "plugin_instance_ids": stem.plugin_instance_ids,
        PROOF_LEVEL_KEY: stem.proof_level.as_str(),
    })
}

fn export_json(export: &ExportEvidence) -> Value {
    json!({
        "file_path": export.file_path,
        "file_name": export.file_name,
        "sha256": export.sha256,
        "format": export.format,
        "file_size_bytes": export.file_size_bytes,
        "duration_seconds": export.duration_seconds,
        "sample_rate_hz": export.sample_rate_hz.clone().map(Value::Number).unwrap_or(Value::Null),
        "channel_count": export.channel_count,
        "exported_at": export.exported_at,
        "export_version": export.export_version,
        PROOF_LEVEL_KEY: ProofLevel::DirectlyObserved.as_str(),
    })
}

fn ingredient_json(ingredient: &IngredientEvidence) -> Value {
    json!({
        "file_name": ingredient.file_name,
        "sha256": ingredient.sha256,
        PROOF_LEVEL_KEY: ingredient.proof_level.as_str(),
        "correlation_confidence": ingredient.correlation_confidence,
        "audio_fingerprint": ingredient.audio_fingerprint.clone().unwrap_or(Value::Null),
    })
}

/// Maps internal edit types to the C2PA action vocabulary:
/// `c2pa.created`, `c2pa.edited`, `c2pa.published`, `c2pa.opened`,
/// `c2pa.placed`, `c2pa.removed`, `c2pa.unknown`.
pub fn c2pa_action_type(edit_type: &str) -> &'static str {
    match edit_type {
        "clip_paste" => "c2pa.placed",
        "clip_delete" => "c2pa.removed",
        "effect_change" => "c2pa.edited",
        "sample_import_confirmed" => "c2pa.placed",
        "arrangement_edit" => "c2pa.edited",
        "undo" => "c2pa.edited",
        _ => "c2pa.unknown",
    }
}
