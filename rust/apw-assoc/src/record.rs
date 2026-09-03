use apw_core::{AssociationStatus, ProofLevel, PROOF_LEVEL_KEY};
use serde_json::{json, Map, Value};

use crate::feature::ExtractionDetails;
use crate::pyround::python_round;
use crate::similarity::ALIGNMENT_THRESHOLD;

pub const METHOD: &str = "routed_feature_sequence_offset_search";
pub const METHOD_VERSION: &str = "2.0.0";
pub const MAX_FEATURE_WINDOWS: usize = 12_000;
pub const ROUTED_FEATURE_SOURCE: &str =
    "accepted buffer_hash events emitted from routed plug-in observations";
pub const FEATURE_DIMENSIONS: [&str; 4] = [
    "relative_rms",
    "zero_crossing_rate",
    "crest_factor",
    "energy_envelope_4",
];
pub const COMPARISON_LIMITATIONS: [&str; 4] = [
    "This bounded feature comparison is not a perceptual watermark, identity system, or registry lookup.",
    "Gain normalization tolerates fixed level changes, but mastering, edits, silence, and channel mixing can reduce confidence.",
    "A failed or unavailable match does not prove that routed audio is absent from the export.",
    "Only the retained bounded routed-feature prefix participates in long-session alignment.",
];
pub const UNAVAILABLE_LIMITATIONS: [&str; 1] = [
    "Unavailable comparison is not evidence that routed audio was absent from the export.",
];
pub const NOT_ESTABLISHED_REASON: &str =
    "bounded routed/export feature similarity did not meet the inference threshold";

/// One rendered point of the alignment chart. `similarity` is the raw score;
/// `to_value` rounds it to three places, while `matched` is decided on the raw
/// score, so a score of 0.7195 renders as 0.72 with `matched` false.
#[derive(Debug, Clone, PartialEq)]
pub struct AlignmentPoint {
    pub relative_window: usize,
    pub similarity: f64,
    pub matched: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    pub overlap_window_count: usize,
    pub routed_window_count: usize,
    pub export_window_count: usize,
    pub best_offset_windows: i64,
    pub best_offset_seconds: f64,
    pub series: Vec<AlignmentPoint>,
}

/// The stem/export association record written into the manifest.
///
/// IMPORTANT: the proof level is derived from the status and is never settable.
/// An established match is `Inferred`; nothing here can ever be
/// `DirectlyObserved`, because the comparison observes bounded features of the
/// export, not the routing that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct AssociationRecord {
    pub status: AssociationStatus,
    pub confidence: Option<f64>,
    // IMPORTANT: None, not zero. Zero coverage reads as "we compared and matched nothing", which
    // inverts absence of evidence into evidence of absence. None says no comparison happened.
    pub matched_coverage: Option<f64>,
    pub routed_coverage: Option<f64>,
    pub matched_window_count: Option<usize>,
    pub comparable_window_count: Option<usize>,
    pub reason: Option<String>,
    pub alignment: Option<Alignment>,
    pub extraction: Option<ExtractionDetails>,
}

impl AssociationRecord {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        AssociationRecord {
            status: AssociationStatus::Unavailable,
            confidence: None,
            matched_coverage: None,
            routed_coverage: None,
            matched_window_count: None,
            comparable_window_count: None,
            reason: Some(reason.into()),
            alignment: None,
            extraction: None,
        }
    }

    pub(crate) fn compared(
        established: bool,
        confidence: f64,
        matched_coverage: f64,
        routed_coverage: f64,
        matched_window_count: usize,
        comparable_window_count: usize,
        alignment: Alignment,
    ) -> Self {
        AssociationRecord {
            status: if established {
                AssociationStatus::InferredMatch
            } else {
                AssociationStatus::NotEstablished
            },
            confidence: Some(confidence),
            matched_coverage: Some(matched_coverage),
            routed_coverage: Some(routed_coverage),
            matched_window_count: Some(matched_window_count),
            comparable_window_count: Some(comparable_window_count),
            reason: if established {
                None
            } else {
                Some(NOT_ESTABLISHED_REASON.to_owned())
            },
            alignment: Some(alignment),
            extraction: None,
        }
    }

    pub fn proof_level(&self) -> ProofLevel {
        self.status.required_proof_level()
    }

    pub fn to_value(&self) -> Value {
        let mut record = Map::new();
        record.insert("status".into(), json!(self.status.as_str()));
        record.insert("method".into(), json!(METHOD));
        record.insert("method_version".into(), json!(METHOD_VERSION));
        match &self.alignment {
            Some(alignment) => self.insert_comparison(&mut record, alignment),
            None => self.insert_unavailable(&mut record),
        }
        if let Some(extraction) = &self.extraction {
            record.insert("export_feature_extraction".into(), extraction.to_value());
            record.insert("routed_feature_source".into(), json!(ROUTED_FEATURE_SOURCE));
            record.insert(
                "bounded_routed_window_limit".into(),
                json!(MAX_FEATURE_WINDOWS),
            );
        }
        Value::Object(record)
    }

    fn insert_comparison(&self, record: &mut Map<String, Value>, alignment: &Alignment) {
        let series: Vec<Value> = alignment
            .series
            .iter()
            .map(|point| {
                json!({
                    "relative_window": point.relative_window,
                    "similarity": python_round(point.similarity, 3),
                    "matched": point.matched,
                })
            })
            .collect();
        let similarity: Vec<Value> = alignment
            .series
            .iter()
            .map(|point| json!(python_round(point.similarity, 3)))
            .collect();
        record.insert(
            "confidence".into(),
            match self.confidence {
                Some(value) => json!(python_round(value, 4)),
                None => Value::Null,
            },
        );
        record.insert(
            "matched_coverage".into(),
            match self.matched_coverage {
                Some(value) => json!(python_round(value, 4)),
                None => Value::Null,
            },
        );
        record.insert(
            "routed_coverage".into(),
            match self.routed_coverage {
                Some(value) => json!(python_round(value.min(1.0), 4)),
                None => Value::Null,
            },
        );
        record.insert(
            "matched_window_count".into(),
            json!(self.matched_window_count),
        );
        record.insert(
            "comparable_window_count".into(),
            json!(self.comparable_window_count),
        );
        record.insert(
            "overlap_window_count".into(),
            json!(alignment.overlap_window_count),
        );
        record.insert(
            "routed_window_count".into(),
            json!(alignment.routed_window_count),
        );
        record.insert(
            "export_window_count".into(),
            json!(alignment.export_window_count),
        );
        record.insert(
            "best_offset_windows".into(),
            json!(alignment.best_offset_windows),
        );
        record.insert(
            "best_offset_seconds".into(),
            json!(python_round(alignment.best_offset_seconds, 4)),
        );
        record.insert("alignment_threshold".into(), json!(ALIGNMENT_THRESHOLD));
        record.insert("alignment_series".into(), Value::Array(series));
        record.insert("alignment_similarity".into(), Value::Array(similarity));
        record.insert("feature_dimensions".into(), json!(FEATURE_DIMENSIONS));
        record.insert(
            "reason".into(),
            match &self.reason {
                Some(reason) => json!(reason),
                None => Value::Null,
            },
        );
        record.insert(PROOF_LEVEL_KEY.into(), json!(self.proof_level().as_str()));
        record.insert("limitations".into(), json!(COMPARISON_LIMITATIONS));
    }

    fn insert_unavailable(&self, record: &mut Map<String, Value>) {
        record.insert("confidence".into(), Value::Null);
        record.insert("matched_coverage".into(), Value::Null);
        record.insert("routed_coverage".into(), Value::Null);
        record.insert("matched_window_count".into(), Value::Null);
        record.insert("comparable_window_count".into(), Value::Null);
        record.insert(
            "reason".into(),
            match &self.reason {
                Some(reason) => json!(reason),
                None => Value::Null,
            },
        );
        record.insert("alignment_series".into(), Value::Array(Vec::new()));
        record.insert("alignment_similarity".into(), Value::Array(Vec::new()));
        record.insert(PROOF_LEVEL_KEY.into(), json!(self.proof_level().as_str()));
        record.insert("limitations".into(), json!(UNAVAILABLE_LIMITATIONS));
    }
}
