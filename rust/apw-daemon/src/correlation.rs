use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apw_core::{
    append_jsonl, canonical_json_ascii, python_str, DEFAULT_EVIDENCE_BACKUPS,
    DEFAULT_EVIDENCE_MAX_BYTES,
};
use serde_json::{json, Map, Value};

use crate::util::python_round;

/// A single event from any observation layer, timestamped on the daemon clock.
#[derive(Debug, Clone)]
pub struct LayerEvent {
    pub layer: String,
    pub event_type: String,
    pub timestamp_ms: i64,
    pub data: Value,
}

impl LayerEvent {
    pub fn new(
        layer: impl Into<String>,
        event_type: impl Into<String>,
        timestamp_ms: i64,
        data: Value,
    ) -> LayerEvent {
        LayerEvent {
            layer: layer.into(),
            event_type: event_type.into(),
            timestamp_ms,
            data,
        }
    }

    fn field(&self, key: &str) -> Option<&Value> {
        self.data.as_object().and_then(|map| map.get(key))
    }

    fn field_is(&self, key: &str, expected: &str) -> bool {
        self.field(key).and_then(Value::as_str) == Some(expected)
    }
}

/// An edit inferred from cross-layer correlation. Never `directly_observed`: the
/// daemon sees corroborating signals, not the DAW operation itself.
#[derive(Debug, Clone, PartialEq)]
pub struct CompositeEdit {
    pub edit_type: String,
    pub confidence: f64,
    pub timestamp_ms: i64,
    pub contributing_events: Vec<Value>,
    pub notes: Vec<String>,
}

impl CompositeEdit {
    pub fn to_event(&self) -> Value {
        json!({
            "event_type": "composite_edit",
            "proof_level": apw_core::ProofLevel::Inferred.as_str(),
            "timestamp_ms": self.timestamp_ms,
            "edit_type": self.edit_type,
            "confidence": python_round(self.confidence, 3),
            "contributing_events": self.contributing_events,
            "notes": self.notes,
        })
    }
}

/// A group of temporally aligned events from multiple layers.
pub struct CorrelationCandidate<'a> {
    pub window_center_ms: i64,
    pub events: &'a [LayerEvent],
}

impl CorrelationCandidate<'_> {
    fn layers_present(&self) -> BTreeSet<&str> {
        self.events.iter().map(|event| event.layer.as_str()).collect()
    }

    fn span_ms(&self) -> i64 {
        if self.events.len() < 2 {
            return 0;
        }
        let max = self.events.iter().map(|e| e.timestamp_ms).max().unwrap_or(0);
        let min = self.events.iter().map(|e| e.timestamp_ms).min().unwrap_or(0);
        max.saturating_sub(min)
    }

    fn any(&self, predicate: impl Fn(&LayerEvent) -> bool) -> bool {
        self.events.iter().any(predicate)
    }

    fn summarize(&self, predicate: impl Fn(usize, &LayerEvent) -> bool) -> Vec<Value> {
        self.events
            .iter()
            .enumerate()
            .filter(|(index, event)| predicate(*index, event))
            .map(|(_, event)| summarize_event(event))
            .collect()
    }
}

fn summarize_event(event: &LayerEvent) -> Value {
    let source_timestamp = match event.field("source_timestamp_ms") {
        Some(value) => value.clone(),
        None => event.field("timestamp_ms").cloned().unwrap_or(Value::Null),
    };
    let event_id = match event.field("daemon_event_id") {
        Some(value) => value.clone(),
        None => {
            let sequence = event
                .field("event_sequence")
                .map(python_str)
                .unwrap_or_default();
            json!(format!(
                "{}:{}:{}:{sequence}",
                event.layer, event.event_type, event.timestamp_ms
            ))
        }
    };
    json!({
        "layer": event.layer,
        "event_type": event.event_type,
        "timestamp_ms": event.timestamp_ms,
        "source_timestamp_ms": source_timestamp,
        "event_id": event_id,
    })
}

fn alignment_bonus(candidate: &CorrelationCandidate) -> f64 {
    let span = candidate.span_ms();
    if span < 200 {
        0.05
    } else if span > 1000 {
        -0.10
    } else {
        0.0
    }
}

fn layer_bonus(candidate: &CorrelationCandidate, required: i64) -> f64 {
    let extra = candidate.layers_present().len() as i64 - required;
    (extra as f64 * 0.05).max(0.0)
}

/// Continuous audio-observation streams: they corroborate an action but never
/// identify one, so they stay out of composite-edit dedup keys.
const SUPPORTING_EVENT_TYPES: [&str; 4] = [
    "buffer_hash",
    "audio_transition",
    "spectral_shift",
    "spectral_profile_change",
];

pub trait CorrelationRule: Send + Sync {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit>;
}

/// TODO: cannot fire until the input-capture layer emits `probable_operation`
/// shortcuts; kept so the rule set stays identical to the Python engine.
pub struct ClipPasteRule;
impl CorrelationRule for ClipPasteRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_paste = candidate.any(|event| {
            event.layer == "input_capture" && event.field_is("probable_operation", "paste")
        });
        let has_audio_start = candidate.any(|event| {
            event.layer == "audio_buffer"
                && event.event_type == "audio_transition"
                && event.field_is("direction", "silence_to_audio")
        });
        if !(has_paste && has_audio_start) {
            return None;
        }
        let confidence = 0.85 + alignment_bonus(candidate) + layer_bonus(candidate, 2);
        Some(CompositeEdit {
            edit_type: "clip_paste".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                (event.layer == "input_capture" && event.field_is("probable_operation", "paste"))
                    || (event.event_type == "audio_transition"
                        && event.field_is("direction", "silence_to_audio"))
            }),
            notes: Vec::new(),
        })
    }
}

/// TODO: cannot fire until the input-capture layer is implemented.
pub struct ClipDeleteRule;
impl CorrelationRule for ClipDeleteRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_delete = candidate.any(|event| {
            event.layer == "input_capture" && event.field_is("probable_operation", "delete")
        });
        let has_audio_stop = candidate.any(|event| {
            event.layer == "audio_buffer"
                && event.event_type == "audio_transition"
                && event.field_is("direction", "audio_to_silence")
        });
        if !(has_delete && has_audio_stop) {
            return None;
        }
        let confidence = 0.80 + alignment_bonus(candidate) + layer_bonus(candidate, 2);
        Some(CompositeEdit {
            edit_type: "clip_delete".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                (event.layer == "input_capture" && event.field_is("probable_operation", "delete"))
                    || (event.event_type == "audio_transition"
                        && event.field_is("direction", "audio_to_silence"))
            }),
            notes: Vec::new(),
        })
    }
}

/// TODO: cannot fire until the screen-observer layer is implemented.
pub struct EffectChangeRule;
impl CorrelationRule for EffectChangeRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_mixer_change = candidate.any(|event| {
            event.layer == "screen_observer" && event.event_type == "screen_mixer_changed"
        });
        let has_spectral_shift = candidate
            .any(|event| event.layer == "audio_buffer" && event.event_type == "spectral_shift");
        let has_silence_transition = candidate
            .any(|event| event.layer == "audio_buffer" && event.event_type == "audio_transition");
        if !(has_mixer_change && has_spectral_shift && !has_silence_transition) {
            return None;
        }
        let confidence = 0.70 + alignment_bonus(candidate) + layer_bonus(candidate, 2);
        Some(CompositeEdit {
            edit_type: "effect_change".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                event.event_type == "screen_mixer_changed" || event.event_type == "spectral_shift"
            }),
            notes: Vec::new(),
        })
    }
}

pub struct ParameterAdjustRule;
impl CorrelationRule for ParameterAdjustRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_param = candidate
            .any(|event| event.layer == "midi" && event.event_type == "parameter_change");
        let has_profile = candidate.any(|event| {
            event.layer == "audio_buffer" && event.event_type == "spectral_profile_change"
        });
        if !(has_param && has_profile) {
            return None;
        }
        let cc = candidate
            .events
            .iter()
            .find(|event| event.event_type == "parameter_change")
            .and_then(|event| event.field("cc_number").map(python_str))
            .unwrap_or_else(|| "?".to_owned());
        let confidence = 0.80 + alignment_bonus(candidate) + layer_bonus(candidate, 2);
        Some(CompositeEdit {
            edit_type: "effect_adjusted".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                event.event_type == "parameter_change"
                    || event.event_type == "spectral_profile_change"
            }),
            notes: vec![format!(
                "CC#{cc} change correlated with spectral profile shift"
            )],
        })
    }
}

pub struct RecordingStartRule;
impl CorrelationRule for RecordingStartRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_transport_play = candidate.any(|event| {
            event.layer == "transport"
                && event.event_type == "transport_change"
                && (event.field_is("transport_state", "playing")
                    || event.field_is("transport_state", "recording"))
        });
        let has_audio_start = candidate.any(|event| {
            event.layer == "audio_buffer"
                && event.event_type == "audio_transition"
                && event.field_is("direction", "silence_to_audio")
        });
        if !(has_transport_play && has_audio_start) {
            return None;
        }
        let is_recording = candidate.any(|event| {
            event.event_type == "transport_change"
                && event.field_is("transport_state", "recording")
        });
        let confidence = 0.85 + alignment_bonus(candidate);
        Some(CompositeEdit {
            edit_type: if is_recording {
                "recording_started".to_owned()
            } else {
                "playback_started".to_owned()
            },
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                (event.event_type == "transport_change"
                    && (event.field_is("transport_state", "playing")
                        || event.field_is("transport_state", "recording")))
                    || (event.event_type == "audio_transition"
                        && event.field_is("direction", "silence_to_audio"))
            }),
            notes: Vec::new(),
        })
    }
}

pub struct ContentChangeRule;
impl CorrelationRule for ContentChangeRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_spectral_shift = candidate
            .any(|event| event.layer == "audio_buffer" && event.event_type == "spectral_shift");
        let has_transition = candidate
            .any(|event| event.layer == "audio_buffer" && event.event_type == "audio_transition");
        if !has_spectral_shift || has_transition {
            return None;
        }
        let has_profile_change =
            candidate.any(|event| event.event_type == "spectral_profile_change");
        let mut confidence = 0.55;
        if has_profile_change {
            confidence += 0.10;
        }
        confidence += layer_bonus(candidate, 1);
        Some(CompositeEdit {
            edit_type: "content_changed".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                event.event_type == "spectral_shift"
                    || event.event_type == "spectral_profile_change"
            }),
            notes: Vec::new(),
        })
    }
}

fn project_diff_events<'a>(
    candidate: &'a CorrelationCandidate,
) -> impl Iterator<Item = (usize, &'a LayerEvent)> {
    candidate
        .events
        .iter()
        .enumerate()
        .filter(|(_, event)| event.layer == "project_differ" && event.event_type == "project_diff")
}

pub struct DeviceAddedRule;
impl CorrelationRule for DeviceAddedRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        for (index, event) in project_diff_events(candidate) {
            let devices = match event.field("devices_changed") {
                Some(Value::Array(items)) if !items.is_empty() => items.clone(),
                _ => continue,
            };
            let confidence = 0.70 + layer_bonus(candidate, 1);
            let rendered: Vec<String> = devices.iter().take(5).map(python_str).collect();
            return Some(CompositeEdit {
                edit_type: "device_chain_changed".to_owned(),
                confidence: confidence.min(1.0),
                timestamp_ms: candidate.window_center_ms,
                contributing_events: candidate.summarize(|position, _| position == index),
                notes: vec![format!("Devices changed on: {}", rendered.join(", "))],
            });
        }
        None
    }
}

pub struct AutomationEditRule;
impl CorrelationRule for AutomationEditRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        for (index, event) in project_diff_events(candidate) {
            let delta = event.field("automation_points_delta").and_then(Value::as_i64);
            let delta = match delta {
                Some(delta) if delta != 0 => delta,
                _ => continue,
            };
            let confidence = 0.65 + layer_bonus(candidate, 1);
            return Some(CompositeEdit {
                edit_type: "automation_edited".to_owned(),
                confidence: confidence.min(1.0),
                timestamp_ms: candidate.window_center_ms,
                contributing_events: candidate.summarize(|position, _| position == index),
                notes: vec![format!("Automation points delta: {delta:+}")],
            });
        }
        None
    }
}

pub struct MidiEditRule;
impl CorrelationRule for MidiEditRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        for (index, event) in project_diff_events(candidate) {
            let delta = event.field("midi_notes_delta").and_then(Value::as_i64);
            let delta = match delta {
                Some(delta) if delta != 0 => delta,
                _ => continue,
            };
            let confidence = 0.65 + layer_bonus(candidate, 1);
            return Some(CompositeEdit {
                edit_type: "midi_edited".to_owned(),
                confidence: confidence.min(1.0),
                timestamp_ms: candidate.window_center_ms,
                contributing_events: candidate.summarize(|position, _| position == index),
                notes: vec![format!("MIDI notes delta: {delta:+}")],
            });
        }
        None
    }
}

pub struct SampleImportRule;
impl CorrelationRule for SampleImportRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let (sample_index, sample_event) = candidate.events.iter().enumerate().find(|(_, event)| {
            event.layer == "sample_watcher" && event.event_type == "sample_file_observed"
        })?;
        let has_project_ref = candidate.any(|event| {
            event.layer == "project_differ" && event.event_type == "project_sample_ref_added"
        });
        let has_correlation = candidate.any(|event| {
            event.layer == "audio_buffer" && event.event_type == "ingredient_correlation"
        });
        let mut confidence = 0.60;
        if has_project_ref {
            confidence += 0.15;
        }
        if has_correlation {
            confidence += 0.15;
        }
        confidence += layer_bonus(candidate, 1);
        let file_name = sample_event
            .field("file_name")
            .map(python_str)
            .unwrap_or_else(|| "unknown".to_owned());
        Some(CompositeEdit {
            edit_type: "sample_import_confirmed".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|position, event| {
                position == sample_index
                    || event.event_type == "project_sample_ref_added"
                    || event.event_type == "ingredient_correlation"
            }),
            notes: vec![format!("Sample: {file_name}")],
        })
    }
}

/// TODO: cannot fire until the input-capture layer is implemented, and hash-chain
/// rollback detection is still missing, which is why the note is emitted.
pub struct UndoRule;
impl CorrelationRule for UndoRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_undo = candidate.any(|event| {
            event.layer == "input_capture" && event.field_is("probable_operation", "undo")
        });
        if !has_undo {
            return None;
        }
        let confidence = 0.75 + alignment_bonus(candidate);
        Some(CompositeEdit {
            edit_type: "undo".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                event.layer == "input_capture" && event.field_is("probable_operation", "undo")
            }),
            notes: vec!["Hash chain rollback detection not yet implemented.".to_owned()],
        })
    }
}

pub struct ArrangementEditRule;
impl CorrelationRule for ArrangementEditRule {
    fn evaluate(&self, candidate: &CorrelationCandidate) -> Option<CompositeEdit> {
        let has_project_diff = candidate.any(|event| {
            event.layer == "project_differ" && event.event_type == "project_diff"
        });
        if !has_project_diff {
            return None;
        }
        let has_screen_change = candidate.any(|event| {
            event.layer == "screen_observer" && event.event_type == "screen_arrangement_changed"
        });
        let has_input = candidate.any(|event| event.layer == "input_capture");

        let mut confidence = 0.65;
        if has_screen_change {
            confidence += 0.05;
        }
        if has_input {
            confidence += 0.05;
        }
        confidence += layer_bonus(candidate, 1);

        let diff = project_diff_events(candidate)
            .next()
            .and_then(|(_, event)| event.data.as_object().cloned())
            .unwrap_or_default();
        let counter = |key: &str| -> String {
            diff.get(key).map(python_str).unwrap_or_else(|| "0".to_owned())
        };
        Some(CompositeEdit {
            edit_type: "arrangement_edit".to_owned(),
            confidence: confidence.min(1.0),
            timestamp_ms: candidate.window_center_ms,
            contributing_events: candidate.summarize(|_, event| {
                event.event_type == "project_diff"
                    || event.event_type == "screen_arrangement_changed"
                    || event.layer == "input_capture"
            }),
            notes: vec![format!(
                "clips +{}/-{}/~{}",
                counter("clips_added"),
                counter("clips_removed"),
                counter("clips_modified")
            )],
        })
    }
}

pub fn default_rules() -> Vec<Box<dyn CorrelationRule>> {
    vec![
        Box::new(ClipPasteRule),
        Box::new(ClipDeleteRule),
        Box::new(EffectChangeRule),
        Box::new(ParameterAdjustRule),
        Box::new(RecordingStartRule),
        Box::new(ContentChangeRule),
        Box::new(DeviceAddedRule),
        Box::new(AutomationEditRule),
        Box::new(MidiEditRule),
        Box::new(SampleImportRule),
        Box::new(UndoRule),
        Box::new(ArrangementEditRule),
    ]
}

#[derive(Debug, Default)]
struct CorrelationState {
    buffer: VecDeque<LayerEvent>,
    dedup_keys: HashSet<String>,
    dedup_order: VecDeque<String>,
    emitted_count: u64,
    capacity_drops: u64,
    duplicate_suppressions: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CorrelationDiagnostics {
    pub buffer_events: usize,
    pub max_buffer_events: usize,
    pub capacity_drops: u64,
    pub duplicate_suppressions: u64,
    pub emitted_count: u64,
}

impl CorrelationDiagnostics {
    pub fn to_json(&self) -> Value {
        json!({
            "buffer_events": self.buffer_events,
            "buffer_max_events": self.max_buffer_events,
            "capacity_drops": self.capacity_drops,
            "duplicate_matches_suppressed": self.duplicate_suppressions,
            "composite_events_emitted": self.emitted_count,
            "clock": "daemon_monotonic_ms",
        })
    }
}

/// Sliding-window cross-layer correlation.
///
/// LOCK ORDER: `state`, then `write`, and never both at once. `ingest` computes
/// under `state`, releases it, then appends evidence under `write`; the append is
/// serialised because `append_jsonl`'s check-then-rotate is not safe under
/// concurrent writers.
pub struct CorrelationEngine {
    window_ms: i64,
    rules: Vec<Box<dyn CorrelationRule>>,
    evidence_path: PathBuf,
    max_buffer_events: usize,
    max_dedup_keys: usize,
    evidence_max_bytes: u64,
    evidence_backups: u32,
    state: Mutex<CorrelationState>,
    write: Mutex<()>,
}

impl CorrelationEngine {
    pub fn new(window_ms: i64, evidence_path: &Path) -> CorrelationEngine {
        CorrelationEngine {
            window_ms,
            rules: default_rules(),
            evidence_path: evidence_path.to_path_buf(),
            max_buffer_events: 512,
            max_dedup_keys: 4096,
            evidence_max_bytes: DEFAULT_EVIDENCE_MAX_BYTES,
            evidence_backups: DEFAULT_EVIDENCE_BACKUPS,
            state: Mutex::new(CorrelationState::default()),
            write: Mutex::new(()),
        }
    }

    pub fn with_bounds(mut self, max_buffer_events: usize, max_dedup_keys: usize) -> Self {
        self.max_buffer_events = max_buffer_events.max(2);
        self.max_dedup_keys = max_dedup_keys.max(32);
        self
    }

    pub fn with_rules(mut self, rules: Vec<Box<dyn CorrelationRule>>) -> Self {
        self.rules = rules;
        self
    }

    /// Adds an event and returns the composite edits it triggered, having already
    /// appended each to the composite evidence log.
    pub fn ingest(&self, event: LayerEvent) -> Vec<CompositeEdit> {
        let results = {
            let Ok(mut state) = self.state.lock() else {
                return Vec::new();
            };
            let now_ms = event.timestamp_ms;
            state.buffer.push_back(event);
            let cutoff = now_ms.saturating_sub(self.window_ms);
            while state
                .buffer
                .front()
                .is_some_and(|oldest| oldest.timestamp_ms < cutoff)
            {
                state.buffer.pop_front();
            }
            while state.buffer.len() > self.max_buffer_events {
                state.buffer.pop_front();
                state.capacity_drops = state.capacity_drops.saturating_add(1);
                if state.capacity_drops == 1 || state.capacity_drops % 1000 == 0 {
                    log::warn!(
                        "Correlation buffer capacity reached; dropped={} max_events={}",
                        state.capacity_drops,
                        self.max_buffer_events
                    );
                }
            }
            self.evaluate(&mut state)
        };

        if results.is_empty() {
            return results;
        }
        let Ok(_writing) = self.write.lock() else {
            return results;
        };
        for composite in &results {
            if let Err(error) = append_jsonl(
                &self.evidence_path,
                &composite.to_event(),
                self.evidence_max_bytes,
                self.evidence_backups,
            ) {
                log::error!("Could not append composite edit evidence: {error}");
            }
        }
        results
    }

    fn evaluate(&self, state: &mut CorrelationState) -> Vec<CompositeEdit> {
        if state.buffer.len() < 2 {
            return Vec::new();
        }
        let events: Vec<LayerEvent> = state.buffer.iter().cloned().collect();
        let window_center_ms = events.last().map(|event| event.timestamp_ms).unwrap_or(0);
        let candidate = CorrelationCandidate {
            window_center_ms,
            events: &events,
        };

        let mut results = Vec::new();
        for rule in &self.rules {
            let Some(composite) = rule.evaluate(&candidate) else {
                continue;
            };
            if composite.confidence < 0.5 || composite.contributing_events.is_empty() {
                continue;
            }
            let key = self.dedup_key(&composite);
            if state.dedup_keys.contains(&key) {
                state.duplicate_suppressions = state.duplicate_suppressions.saturating_add(1);
                continue;
            }
            state.dedup_keys.insert(key.clone());
            state.dedup_order.push_back(key);
            while state.dedup_order.len() > self.max_dedup_keys {
                if let Some(expired) = state.dedup_order.pop_front() {
                    state.dedup_keys.remove(&expired);
                }
            }
            state.emitted_count = state.emitted_count.saturating_add(1);
            results.push(composite);
        }
        results
    }

    /// IMPORTANT: keying on the full contributing set let the same action re-emit
    /// on every new supporting audio event, because the set (and so the key) grew
    /// each ingest. The action's identity is its non-continuous trigger events;
    /// continuous audio evidence only corroborates.
    fn dedup_key(&self, composite: &CompositeEdit) -> String {
        let mut anchors: Vec<String> = composite
            .contributing_events
            .iter()
            .filter(|item| {
                let event_type = item
                    .get("event_type")
                    .map(python_str)
                    .unwrap_or_default();
                !SUPPORTING_EVENT_TYPES.contains(&event_type.as_str())
            })
            .map(|item| item.get("event_id").map(python_str).unwrap_or_default())
            .collect();
        anchors.sort();
        if anchors.is_empty() {
            // All-supporting rules (content_changed) have no identifying event, and
            // any event-derived anchor drifts as the window slides. Quantize
            // instead: one emission per window span per edit type.
            anchors = vec![format!(
                "window:{}",
                composite.timestamp_ms.div_euclid(self.window_ms.max(1))
            )];
        }
        let key = json!([composite.edit_type, anchors]);
        canonical_json_ascii(&key)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| format!("{}:{:?}", composite.edit_type, anchors))
    }

    pub fn diagnostics(&self) -> CorrelationDiagnostics {
        self.state
            .lock()
            .map(|state| CorrelationDiagnostics {
                buffer_events: state.buffer.len(),
                max_buffer_events: self.max_buffer_events,
                capacity_drops: state.capacity_drops,
                duplicate_suppressions: state.duplicate_suppressions,
                emitted_count: state.emitted_count,
            })
            .unwrap_or(CorrelationDiagnostics {
                max_buffer_events: self.max_buffer_events,
                ..CorrelationDiagnostics::default()
            })
    }

    /// Clears the sliding window and the dedup ledger for a new capture session.
    pub fn reset(&self) {
        if let Ok(mut state) = self.state.lock() {
            *state = CorrelationState::default();
        }
    }
}

pub(crate) fn object_of(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}
