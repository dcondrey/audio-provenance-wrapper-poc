use std::path::Path;

use apw_core::{sha256_file, ProofLevel, APW_VERSION, PROOF_LEVEL_KEY};
use serde_json::{json, Map, Value};

use crate::error::{C2paError, Result};

pub const PLUGIN_NAME: &str = "audio-provenance-wrapper";
// REQUIRED: this reaches softwareAgent.version and claim_generator_info inside the SIGNED
// manifest, so it must track the one version constant rather than drift from it.
pub const PLUGIN_VERSION: &str = APW_VERSION;

pub const ACTIONS_ASSERTION_LABEL: &str = "c2pa.actions.v2";
pub const UNOBSERVED_ASSERTION_LABEL: &str = "apw.unobserved";
pub const SHA256_KEY: &str = "apw:sha256";

const DIGITAL_SOURCE_TYPE_BASE: &str = "http://cv.iptc.org/newscodes/digitalsourcetype/";
pub const DIGITAL_CAPTURE: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/digitalCapture";
pub const COMPOSITE_CAPTURE: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/compositeCapture";
pub const ALGORITHMICALLY_ENHANCED: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/algorithmicallyEnhanced";
pub const MINOR_HUMAN_EDITS: &str =
    "http://cv.iptc.org/newscodes/digitalsourcetype/minorHumanEdits";

const UNKNOWN_ACTION: (&str, &str) = ("c2pa.unknown", MINOR_HUMAN_EDITS);

const EDIT_ACTIONS: [(&str, (&str, &str)); 7] = [
    ("clip_paste", ("c2pa.placed", COMPOSITE_CAPTURE)),
    ("clip_delete", ("c2pa.removed", MINOR_HUMAN_EDITS)),
    ("effect_change", ("c2pa.edited", ALGORITHMICALLY_ENHANCED)),
    ("sample_import_confirmed", ("c2pa.placed", COMPOSITE_CAPTURE)),
    ("arrangement_edit", ("c2pa.edited", MINOR_HUMAN_EDITS)),
    ("undo", ("c2pa.edited", MINOR_HUMAN_EDITS)),
    ("recorded", ("c2pa.created", DIGITAL_CAPTURE)),
];

const INGREDIENT_LINKED_ACTIONS: [&str; 3] = ["c2pa.placed", "c2pa.opened", "c2pa.removed"];

pub fn digital_source_type_base() -> &'static str {
    DIGITAL_SOURCE_TYPE_BASE
}

pub fn action_for_edit_type(edit_type: &str) -> (&'static str, &'static str) {
    EDIT_ACTIONS
        .iter()
        .find(|(key, _)| *key == edit_type)
        .map(|(_, action)| *action)
        .unwrap_or(UNKNOWN_ACTION)
}

fn software_agent() -> Value {
    json!({"name": PLUGIN_NAME, "version": PLUGIN_VERSION})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum IngredientRelationship {
    #[serde(rename = "componentOf")]
    ComponentOf,
    #[serde(rename = "inputTo")]
    InputTo,
    #[serde(rename = "parentOf")]
    ParentOf,
}

impl IngredientRelationship {
    pub fn as_str(self) -> &'static str {
        match self {
            IngredientRelationship::ComponentOf => "componentOf",
            IngredientRelationship::InputTo => "inputTo",
            IngredientRelationship::ParentOf => "parentOf",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "componentOf" => Ok(IngredientRelationship::ComponentOf),
            "inputTo" => Ok(IngredientRelationship::InputTo),
            "parentOf" => Ok(IngredientRelationship::ParentOf),
            other => Err(C2paError::Manifest(format!(
                "Unknown ingredient relationship {other:?}; expected one of \
                 [\"componentOf\", \"inputTo\", \"parentOf\"]"
            ))),
        }
    }
}

fn slug(value: &str) -> Result<String> {
    let cleaned: String = value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        return Err(C2paError::Manifest(
            "Ingredient titles must contain at least one usable character".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn validate_sha256(title: &str, digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(C2paError::Manifest(format!(
            "Ingredient {title:?} recorded a sha256 that is not 64 hex characters; an \
             ingredient without provenance must still record a usable hash"
        )));
    }
    Ok(())
}

/// One input asset referenced by the signed manifest.
///
/// IMPORTANT: every ingredient carries its own [`ProofLevel`]. An ingredient with no
/// provenance of its own is `unknown_unobserved` and records only its hash; it never
/// borrows the export's proof level and it makes no provenance claim of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingredient {
    title: String,
    relationship: IngredientRelationship,
    proof_level: ProofLevel,
    sha256: String,
    mime: String,
    ingredient_id: String,
}

impl Ingredient {
    pub fn declared(
        title: impl Into<String>,
        relationship: IngredientRelationship,
        proof_level: ProofLevel,
        sha256: impl Into<String>,
    ) -> Result<Self> {
        let title = title.into();
        let sha256 = sha256.into();
        validate_sha256(&title, &sha256)?;
        let ingredient_id = slug(&title)?;
        Ok(Self {
            title,
            relationship,
            proof_level,
            sha256,
            mime: "audio/wav".to_string(),
            ingredient_id,
        })
    }

    pub fn from_file(
        title: impl Into<String>,
        relationship: IngredientRelationship,
        proof_level: ProofLevel,
        source_path: &Path,
    ) -> Result<Self> {
        let digest = sha256_file(source_path)?;
        Self::declared(title, relationship, proof_level, digest)
    }

    pub fn with_id(mut self, ingredient_id: impl Into<String>) -> Result<Self> {
        let id = ingredient_id.into();
        if id.is_empty() {
            return Err(C2paError::Manifest(
                "An explicit ingredient_id cannot be empty".to_string(),
            ));
        }
        self.ingredient_id = id;
        Ok(self)
    }

    pub fn with_mime(mut self, mime: impl Into<String>) -> Self {
        self.mime = mime.into();
        self
    }

    pub fn id(&self) -> &str {
        &self.ingredient_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn relationship(&self) -> IngredientRelationship {
        self.relationship
    }

    pub fn proof_level(&self) -> ProofLevel {
        self.proof_level
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    pub fn to_value(&self) -> Value {
        json!({
            "title": self.title,
            "label": self.ingredient_id,
            "format": self.mime,
            "relationship": self.relationship.as_str(),
            "metadata": {
                PROOF_LEVEL_KEY: self.proof_level.as_str(),
                SHA256_KEY: self.sha256,
            },
        })
    }
}

/// An ingredient the engine never observed being created: hash recorded, no provenance claimed.
pub fn unobserved_ingredient(title: impl Into<String>, source_path: &Path) -> Result<Ingredient> {
    Ingredient::from_file(
        title,
        IngredientRelationship::InputTo,
        ProofLevel::UnknownUnobserved,
        source_path,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservedAction {
    pub edit_type: String,
    pub proof_level: ProofLevel,
    pub when: Option<String>,
    pub confidence: Option<f64>,
    pub ingredient_ids: Vec<String>,
}

impl ObservedAction {
    pub fn new(edit_type: impl Into<String>, proof_level: ProofLevel) -> Self {
        Self {
            edit_type: edit_type.into(),
            proof_level,
            when: None,
            confidence: None,
            ingredient_ids: Vec::new(),
        }
    }

    pub fn when(mut self, when: impl Into<String>) -> Self {
        self.when = Some(when.into());
        self
    }

    pub fn confidence(mut self, confidence: f64) -> Self {
        self.confidence = Some(confidence);
        self
    }

    pub fn linking(mut self, ingredient_ids: Vec<String>) -> Self {
        self.ingredient_ids = ingredient_ids;
        self
    }

    fn to_value(&self) -> Result<Value> {
        let (action, source_type) = action_for_edit_type(&self.edit_type);
        let action = if INGREDIENT_LINKED_ACTIONS.contains(&action) && self.ingredient_ids.is_empty()
        {
            "c2pa.edited"
        } else {
            action
        };
        let mut entry = Map::new();
        entry.insert("action".to_string(), Value::from(action));
        entry.insert("digitalSourceType".to_string(), Value::from(source_type));
        entry.insert("softwareAgent".to_string(), software_agent());
        if let Some(when) = &self.when {
            entry.insert("when".to_string(), Value::from(when.clone()));
        }
        let mut parameters = Map::new();
        parameters.insert(
            "apw:edit_type".to_string(),
            Value::from(self.edit_type.clone()),
        );
        parameters.insert(
            PROOF_LEVEL_KEY.to_string(),
            Value::from(self.proof_level.as_str()),
        );
        if !self.ingredient_ids.is_empty() {
            parameters.insert(
                "ingredientIds".to_string(),
                Value::from(self.ingredient_ids.clone()),
            );
        }
        if let Some(confidence) = self.confidence {
            let number = serde_json::Number::from_f64(confidence).ok_or_else(|| {
                C2paError::Manifest(format!(
                    "action {:?} carries a non-finite confidence value",
                    self.edit_type
                ))
            })?;
            parameters.insert("apw:confidence".to_string(), Value::Number(number));
        }
        entry.insert("parameters".to_string(), Value::Object(parameters));
        Ok(Value::Object(entry))
    }
}

/// A statement about what the engine could NOT see. Never a provenance claim.
#[derive(Debug, Clone, PartialEq)]
pub struct UnobservedClaim {
    pub claim: String,
    pub value: Value,
    pub evidence: String,
    pub proof_level: ProofLevel,
}

impl UnobservedClaim {
    pub fn new(claim: impl Into<String>, value: Value, evidence: impl Into<String>) -> Self {
        Self {
            claim: claim.into(),
            value,
            evidence: evidence.into(),
            proof_level: ProofLevel::UnknownUnobserved,
        }
    }

    fn to_value(&self) -> Value {
        json!({
            "claim": self.claim,
            "value": self.value,
            "evidence": self.evidence,
            PROOF_LEVEL_KEY: self.proof_level.as_str(),
        })
    }
}

pub const FULL_DAW_PROVENANCE_CLAIM: &str = "full_daw_provenance";

/// REQUIRED: every manifest carries this. The engine observed only the audio routed
/// through the capture plugin, never the whole DAW session.
pub fn full_daw_provenance_unobserved() -> UnobservedClaim {
    UnobservedClaim::new(
        FULL_DAW_PROVENANCE_CLAIM,
        Value::Bool(false),
        "Only audio routed through the capture plugin was observed",
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExtraAssertion {
    pub label: String,
    pub data: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestSpec {
    pub title: String,
    pub actions: Vec<ObservedAction>,
    pub ingredients: Vec<Ingredient>,
    pub unobserved: Vec<UnobservedClaim>,
    pub extra_assertions: Vec<ExtraAssertion>,
    pub mime: String,
}

impl ManifestSpec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            actions: Vec::new(),
            ingredients: Vec::new(),
            unobserved: Vec::new(),
            extra_assertions: Vec::new(),
            mime: "audio/wav".to_string(),
        }
    }
}

fn ingredient_actions(ingredients: &[Ingredient], already_referenced: &[String]) -> Vec<Value> {
    // REQUIRED: claim v2 rejects a c2pa.placed action that references anything but a
    // componentOf ingredient (assertion.action.ingredientMismatch). parentOf ingredients are
    // the lineage of the leading c2pa.opened action; inputTo ingredients stay unreferenced.
    let ids: Vec<String> = ingredients
        .iter()
        .filter(|ing| {
            ing.relationship == IngredientRelationship::ComponentOf
                && !already_referenced.iter().any(|seen| seen == ing.id())
        })
        .map(|ing| ing.id().to_string())
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    vec![json!({
        "action": "c2pa.placed",
        "digitalSourceType": COMPOSITE_CAPTURE,
        "softwareAgent": software_agent(),
        "parameters": {"ingredientIds": ids},
    })]
}

pub fn build_manifest(spec: &ManifestSpec) -> Result<Value> {
    let mut unobserved = spec.unobserved.clone();
    if !unobserved
        .iter()
        .any(|item| item.claim == FULL_DAW_PROVENANCE_CLAIM)
    {
        unobserved.insert(0, full_daw_provenance_unobserved());
    }

    let parent_ids: Vec<String> = spec
        .ingredients
        .iter()
        .filter(|ing| ing.relationship == IngredientRelationship::ParentOf)
        .map(|ing| ing.id().to_string())
        .collect();

    let mut actions: Vec<Value> = if parent_ids.is_empty() {
        vec![json!({
            "action": "c2pa.created",
            "digitalSourceType": DIGITAL_CAPTURE,
            "softwareAgent": software_agent(),
        })]
    } else {
        vec![json!({
            "action": "c2pa.opened",
            "digitalSourceType": COMPOSITE_CAPTURE,
            "softwareAgent": software_agent(),
            "parameters": {"ingredientIds": parent_ids.clone()},
        })]
    };
    for action in &spec.actions {
        actions.push(action.to_value()?);
    }

    let mut referenced = parent_ids;
    for action in &spec.actions {
        for id in &action.ingredient_ids {
            if !referenced.iter().any(|seen| seen == id) {
                referenced.push(id.clone());
            }
        }
    }

    let mut known_ids: Vec<&str> = Vec::with_capacity(spec.ingredients.len());
    for ingredient in &spec.ingredients {
        if known_ids.contains(&ingredient.id()) {
            return Err(C2paError::Manifest(format!(
                "Duplicate ingredient id {:?}; give the colliding ingredient an explicit \
                 ingredient_id so actions reference exactly one file",
                ingredient.id()
            )));
        }
        known_ids.push(ingredient.id());
    }
    let mut unknown: Vec<&str> = referenced
        .iter()
        .filter(|id| !known_ids.contains(&id.as_str()))
        .map(|id| id.as_str())
        .collect();
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(C2paError::Manifest(format!(
            "Actions reference unknown ingredient ids: {unknown:?}"
        )));
    }
    actions.extend(ingredient_actions(&spec.ingredients, &referenced));

    let mut assertions = vec![
        json!({"label": ACTIONS_ASSERTION_LABEL, "data": {"actions": actions}}),
        json!({
            "label": UNOBSERVED_ASSERTION_LABEL,
            "data": {"items": unobserved.iter().map(UnobservedClaim::to_value).collect::<Vec<_>>()},
        }),
    ];
    for extra in &spec.extra_assertions {
        if extra.label == ACTIONS_ASSERTION_LABEL || extra.label == UNOBSERVED_ASSERTION_LABEL {
            return Err(C2paError::Manifest(format!(
                "Assertion {} is owned by the engine and cannot be overridden",
                extra.label
            )));
        }
        assertions.push(json!({"label": extra.label, "data": extra.data}));
    }

    let mut manifest = Map::new();
    manifest.insert("title".to_string(), Value::from(spec.title.clone()));
    manifest.insert("format".to_string(), Value::from(spec.mime.clone()));
    manifest.insert(
        "claim_generator_info".to_string(),
        json!([{"name": PLUGIN_NAME, "version": PLUGIN_VERSION}]),
    );
    manifest.insert("assertions".to_string(), Value::Array(assertions));
    if !spec.ingredients.is_empty() {
        manifest.insert(
            "ingredients".to_string(),
            Value::Array(spec.ingredients.iter().map(Ingredient::to_value).collect()),
        );
    }
    Ok(Value::Object(manifest))
}

pub fn assertion_labels(manifest: &Value) -> Vec<String> {
    manifest
        .get("assertions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("label").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
