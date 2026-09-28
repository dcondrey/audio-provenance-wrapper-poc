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

const LAYER_MAP: [(&str, &str); 9] = [
    ("buffer_hash", "audio_buffer"),
    ("audio_transition", "audio_buffer"),
    ("spectral_shift", "audio_buffer"),
    ("spectral_profile_change", "audio_buffer"),
    ("transport_change", "transport"),
    ("midi_event", "midi"),
    ("parameter_change", "midi"),
    ("session_config_change", "session"),
    ("host_environment", "session"),
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
    /// Cumulative plug-in counters that went backwards and were ignored.
    pub telemetry_regressions: u64,
    /// The first host environment the plug-in reported (four normalised keys).
    pub host_environment: Option<Map<String, Value>>,
    pub host_environment_conflicts: u64,
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
    telemetry_regressions: u64,
    host_environment: Option<Map<String, Value>>,
    host_environment_conflicts: u64,
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
            telemetry_regressions: 0,
            host_environment: None,
            host_environment_conflicts: 0,
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
                // IMPORTANT: cumulative counters. A lower value is a spoofed
                // datagram or a restarted instance; it is counted, ignored, and
                // never lowers the recorded loss.
                if self
                    .latest_plugin_telemetry
                    .iter()
                    .any(|(existing, previous)| existing == key && counter < *previous)
                {
                    self.telemetry_regressions = self.telemetry_regressions.saturating_add(1);
                    log::warn!(
                        "Plug-in telemetry counter {key} went backwards; ignoring the lower \
                         value and grading coverage as partial"
                    );
                    continue;
                }
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

        if map.get("event_type").and_then(Value::as_str) == Some("host_environment") {
            self.record_host_environment(&map);
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

    /// Hold the first host environment and count any later identity
    /// disagreement. Only recognition and name are compared: one host routinely
    /// loads the plug-in in two formats at once.
    fn record_host_environment(&mut self, event: &Map<String, Value>) {
        let or_null = |key: &str| {
            let value = event.get(key).cloned().unwrap_or(Value::Null);
            if apw_core::is_truthy(&value) { value } else { Value::Null }
        };
        let mut observed = Map::new();
        observed.insert(
            "host_recognised".to_owned(),
            Value::Bool(event.get("host_recognised").is_some_and(apw_core::is_truthy)),
        );
        observed.insert("host_name".to_owned(), or_null("host_name"));
        observed.insert("host_executable_name".to_owned(), or_null("host_executable_name"));
        observed.insert("wrapper_format".to_owned(), or_null("wrapper_format"));
        match &self.host_environment {
            None => self.host_environment = Some(observed),
            Some(first) => {
                let differs = ["host_recognised", "host_name"].iter().any(|key| {
                    !apw_core::python_eq(
                        observed.get(*key).unwrap_or(&Value::Null),
                        first.get(*key).unwrap_or(&Value::Null),
                    )
                });
                if differs {
                    self.host_environment_conflicts =
                        self.host_environment_conflicts.saturating_add(1);
                    log::warn!(
                        "Host environment reported differently after the first report; the \
                         manifest will record the host as unobserved"
                    );
                }
            }
        }
    }

    pub fn telemetry_regressions(&self) -> u64 {
        self.telemetry_regressions
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
            telemetry_regressions: self.telemetry_regressions,
            host_environment: self.host_environment.clone(),
            host_environment_conflicts: self.host_environment_conflicts,
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
