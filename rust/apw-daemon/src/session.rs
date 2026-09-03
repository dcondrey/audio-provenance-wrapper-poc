use std::collections::{BTreeSet, VecDeque};

use serde_json::{json, Map, Value};

use crate::receiver::ReceiverDiagnostics;

/// Same bound rationale as [`crate::receiver::MAX_TRACKED_STREAMS`]: both tables
/// are keyed by wire-controlled values and rendered into the signed manifest, so
/// unbounded growth is a local-DoS and manifest-spam vector.
pub const MAX_TRACKED_PLUGIN_KEYS: usize = 64;

pub const MAX_SESSION_EVENTS: usize = 50_000;

/// Matches `audio_association.MAX_FEATURE_WINDOWS`: the association comparison
/// reads at most this many routed windows, so retaining more buys nothing.
pub const MAX_FEATURE_WINDOWS: usize = 12_000;

const LAYER_MAP: [(&str, &str); 8] = [
    ("buffer_hash", "audio_buffer"),
    ("audio_transition", "audio_buffer"),
    ("spectral_shift", "audio_buffer"),
    ("spectral_profile_change", "audio_buffer"),
    ("transport_change", "transport"),
    ("midi_event", "midi"),
    ("parameter_change", "midi"),
    ("session_config_change", "session"),
];

pub fn event_type_to_layer(event_type: &str) -> &'static str {
    LAYER_MAP
        .iter()
        .find(|(name, _)| *name == event_type)
        .map(|(_, layer)| *layer)
        .unwrap_or("audio_buffer")
}

/// Everything the manifest assembly reads out of the session under one lock.
#[derive(Debug, Clone, Default)]
pub struct SessionSnapshot {
    pub events: Vec<Value>,
    pub feature_events: Vec<Value>,
    pub first_hash_event: Map<String, Value>,
    pub last_hash_event: Map<String, Value>,
    pub chain_length: u64,
    pub plugin_instance_ids: Vec<String>,
    pub active_layers: BTreeSet<String>,
    pub telemetry: Vec<(String, i64)>,
    pub feature_window_drops: u64,
    pub events_retained: usize,
    pub events_dropped: u64,
}

/// Live capture-session state.
///
/// LOCK ORDER: held by the daemon inside one `Mutex`; it is a leaf. No I/O and no
/// other lock is taken while it is held.
#[derive(Debug)]
pub struct SessionState {
    session_events: VecDeque<Value>,
    session_event_drops: u64,
    feature_events: VecDeque<Value>,
    feature_window_drops: u64,
    buffer_hash_count: u64,
    first_hash_event: Option<Map<String, Value>>,
    last_hash_event: Option<Map<String, Value>>,
    /// Insertion-ordered LRU, least-recent first, bounded at
    /// [`MAX_TRACKED_PLUGIN_KEYS`].
    plugin_instance_ids: Vec<String>,
    latest_plugin_telemetry: Vec<(String, i64)>,
    active_layers: BTreeSet<String>,
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState::new()
    }
}

impl SessionState {
    pub fn new() -> SessionState {
        SessionState {
            session_events: VecDeque::new(),
            session_event_drops: 0,
            feature_events: VecDeque::new(),
            feature_window_drops: 0,
            buffer_hash_count: 0,
            first_hash_event: None,
            last_hash_event: None,
            plugin_instance_ids: Vec::new(),
            latest_plugin_telemetry: Vec::new(),
            active_layers: BTreeSet::from(["sample_watcher".to_owned()]),
        }
    }

    /// IMPORTANT: reconstruction, not field-by-field clearing, so a field added
    /// later cannot survive a session boundary by being forgotten here.
    pub fn reset(&mut self) {
        *self = SessionState::new();
    }

    pub fn append_event(&mut self, event: Value) {
        if self.session_events.len() >= MAX_SESSION_EVENTS {
            self.session_events.pop_front();
            self.session_event_drops = self.session_event_drops.saturating_add(1);
            if self.session_event_drops == 1 || self.session_event_drops % 10_000 == 0 {
                log::warn!(
                    "Session event memory limit reached; dropped={} max={MAX_SESSION_EVENTS}",
                    self.session_event_drops
                );
            }
        }
        self.session_events.push_back(event);
    }

    pub fn record_plugin_event(&mut self, event: &Value, layer: &str) {
        self.append_event(event.clone());
        self.active_layers.insert(layer.to_owned());

        let map = crate::correlation::object_of(event);
        let instance_id = map
            .get("plugin_instance_id")
            .map(apw_core::python_str)
            .unwrap_or_else(|| "unknown_plugin_instance".to_owned());
        touch_lru(&mut self.plugin_instance_ids, instance_id);
        while self.plugin_instance_ids.len() > MAX_TRACKED_PLUGIN_KEYS {
            let evicted = self.plugin_instance_ids.remove(0);
            log::warn!("Plugin instance table full; evicted least-recent id {evicted}");
        }

        if let Some(Value::Object(telemetry)) = map.get("telemetry") {
            for (key, value) in telemetry {
                let Some(counter) = integer_counter(value) else {
                    continue;
                };
                match self
                    .latest_plugin_telemetry
                    .iter()
                    .position(|(existing, _)| existing == key)
                {
                    Some(index) => {
                        let entry = self.latest_plugin_telemetry.remove(index);
                        self.latest_plugin_telemetry.push((entry.0, counter));
                    }
                    None => self
                        .latest_plugin_telemetry
                        .push((key.clone(), counter)),
                }
            }
            while self.latest_plugin_telemetry.len() > MAX_TRACKED_PLUGIN_KEYS {
                self.latest_plugin_telemetry.remove(0);
            }
        }

        if map.get("event_type").and_then(Value::as_str) == Some("buffer_hash") {
            self.buffer_hash_count = self.buffer_hash_count.saturating_add(1);
            if self.first_hash_event.is_none() {
                self.first_hash_event = Some(map.clone());
            }
            self.last_hash_event = Some(map.clone());
            if self.feature_events.len() >= MAX_FEATURE_WINDOWS {
                self.feature_window_drops = self.feature_window_drops.saturating_add(1);
                self.feature_events.pop_front();
            }
            self.feature_events.push_back(Value::Object(map));
        }
    }

    pub fn chain_length(&self) -> u64 {
        self.buffer_hash_count
    }

    pub fn plugin_instance_count(&self) -> usize {
        self.plugin_instance_ids.len()
    }

    pub fn feature_window_drops(&self) -> u64 {
        self.feature_window_drops
    }

    pub fn events_retained(&self) -> usize {
        self.session_events.len()
    }

    pub fn events_dropped(&self) -> u64 {
        self.session_event_drops
    }

    pub fn telemetry(&self) -> Vec<(String, i64)> {
        self.latest_plugin_telemetry.clone()
    }

    pub fn last_window_hash(&self) -> String {
        self.last_hash_event
            .as_ref()
            .and_then(|event| event.get("window_hash"))
            .map(apw_core::python_str)
            .unwrap_or_default()
    }

    pub fn mark_layer_active(&mut self, layer: &str) {
        self.active_layers.insert(layer.to_owned());
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        let mut plugin_instance_ids = self.plugin_instance_ids.clone();
        plugin_instance_ids.sort();
        SessionSnapshot {
            events: self.session_events.iter().cloned().collect(),
            feature_events: self.feature_events.iter().cloned().collect(),
            first_hash_event: self.first_hash_event.clone().unwrap_or_default(),
            last_hash_event: self.last_hash_event.clone().unwrap_or_default(),
            chain_length: self.buffer_hash_count,
            plugin_instance_ids,
            active_layers: self.active_layers.clone(),
            telemetry: self.latest_plugin_telemetry.clone(),
            feature_window_drops: self.feature_window_drops,
            events_retained: self.session_events.len(),
            events_dropped: self.session_event_drops,
        }
    }

    pub fn memory_json(&self) -> Value {
        json!({
            "events_retained": self.session_events.len(),
            "max_events": MAX_SESSION_EVENTS,
            "events_dropped_from_memory_only": self.session_event_drops,
        })
    }
}

pub fn session_diagnostics(
    receiver: &ReceiverDiagnostics,
    correlation: Value,
    session_memory: Value,
    receipt_summary: Value,
) -> Value {
    json!({
        "receiver": receiver.to_json(),
        "correlation": correlation,
        "session_memory": session_memory,
        "daemon_receipt_acknowledgement": receipt_summary,
        apw_core::PROOF_LEVEL_KEY: apw_core::ProofLevel::DirectlyObserved.as_str(),
    })
}

fn touch_lru(entries: &mut Vec<String>, key: String) {
    if let Some(index) = entries.iter().position(|existing| *existing == key) {
        entries.remove(index);
    }
    entries.push(key);
}

/// Python stores only `isinstance(value, int)` telemetry counters. A JSON float
/// or string is ignored rather than coerced, so a malformed plug-in build cannot
/// inject a non-counter into the signed coverage record.
fn integer_counter(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        _ => None,
    }
}
