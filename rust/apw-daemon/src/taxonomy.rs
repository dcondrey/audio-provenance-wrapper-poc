use apw_core::{python_str, ProofLevel};
use serde_json::{Map, Value};

/// Every observation event the plug-in or a daemon-side observer produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    BufferHash,
    AudioTransition,
    SpectralShift,
    TransportChange,
    MidiEvent,
    SessionConfig,
    SpectralProfileChange,
    ParameterChange,
    SampleFileObserved,
    IngredientCorrelation,
    CompositeEdit,
    ForgeryAnalysis,
    ProjectDiff,
    ProjectSaveDetected,
    LayerUnavailable,
}

impl EventType {
    pub const ALL: [EventType; 15] = [
        EventType::BufferHash,
        EventType::AudioTransition,
        EventType::SpectralShift,
        EventType::TransportChange,
        EventType::MidiEvent,
        EventType::SessionConfig,
        EventType::SpectralProfileChange,
        EventType::ParameterChange,
        EventType::SampleFileObserved,
        EventType::IngredientCorrelation,
        EventType::CompositeEdit,
        EventType::ForgeryAnalysis,
        EventType::ProjectDiff,
        EventType::ProjectSaveDetected,
        EventType::LayerUnavailable,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            EventType::BufferHash => "buffer_hash",
            EventType::AudioTransition => "audio_transition",
            EventType::SpectralShift => "spectral_shift",
            EventType::TransportChange => "transport_change",
            EventType::MidiEvent => "midi_event",
            EventType::SessionConfig => "session_config_change",
            EventType::SpectralProfileChange => "spectral_profile_change",
            EventType::ParameterChange => "parameter_change",
            EventType::SampleFileObserved => "sample_file_observed",
            EventType::IngredientCorrelation => "ingredient_correlation",
            EventType::CompositeEdit => "composite_edit",
            EventType::ForgeryAnalysis => "forgery_analysis",
            EventType::ProjectDiff => "project_diff",
            EventType::ProjectSaveDetected => "project_save_detected",
            EventType::LayerUnavailable => "layer_unavailable",
        }
    }

    pub fn parse(value: &str) -> Option<EventType> {
        EventType::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    pub fn required_fields(self) -> &'static [&'static str] {
        match self {
            EventType::BufferHash => {
                &["window_hash", "prev_hash", "rms_level", "zero_crossing_rate"]
            }
            EventType::AudioTransition => &["direction", "boundary_hash"],
            EventType::SpectralShift => {
                &["prev_spectral_centroid_hz", "new_spectral_centroid_hz"]
            }
            EventType::TransportChange => &["transport_state"],
            EventType::MidiEvent => &["midi_event_type", "midi_channel"],
            EventType::SessionConfig => &["sample_rate_hz", "channel_count"],
            EventType::SpectralProfileChange => {
                &["band_low_delta", "band_mid_delta", "band_high_delta"]
            }
            EventType::ParameterChange => &["cc_number", "change_count"],
            EventType::SampleFileObserved => &["sha256", "file_name"],
            EventType::IngredientCorrelation => &["sample_sha256", "confidence"],
            EventType::CompositeEdit => &["edit_type", "confidence", "contributing_events"],
            EventType::ForgeryAnalysis => &["suspicion_score", "flags"],
            EventType::ProjectDiff => &["clips_added", "clips_removed"],
            EventType::ProjectSaveDetected => &["file_hash"],
            EventType::LayerUnavailable => &["layer", "reason"],
        }
    }

    /// IMPORTANT: honesty constraint 1 (labels never stronger than evidence). The
    /// UDP socket cannot authenticate its sender, so a network event may claim at
    /// most what the in-process plug-in observer legitimately asserts for that
    /// event type. `None` means the type is produced only by daemon-side observers
    /// and must never be accepted from the network at all.
    pub fn network_proof_level_cap(self) -> Option<ProofLevel> {
        match self {
            EventType::BufferHash
            | EventType::AudioTransition
            | EventType::SpectralShift
            | EventType::TransportChange
            | EventType::MidiEvent
            | EventType::SessionConfig
            | EventType::SpectralProfileChange
            | EventType::ParameterChange => Some(ProofLevel::DirectlyObserved),
            _ => None,
        }
    }

    /// IMPORTANT: downstream consumers (status readiness, audio association,
    /// sample correlation) do arithmetic on these fields straight off the wire, so
    /// the type invariant is established here at the UDP boundary rather than
    /// re-checked at every consumer.
    pub fn network_numeric_fields(self) -> &'static [&'static str] {
        match self {
            EventType::BufferHash => &[
                "rms_level",
                "zero_crossing_rate",
                "window_size_samples",
                "sample_rate_hz",
                "spectral_centroid_hz",
                "crest_factor",
            ],
            EventType::SpectralShift => {
                &["prev_spectral_centroid_hz", "new_spectral_centroid_hz"]
            }
            EventType::SessionConfig => &["sample_rate_hz", "channel_count"],
            EventType::SpectralProfileChange => {
                &["band_low_delta", "band_mid_delta", "band_high_delta"]
            }
            EventType::ParameterChange => &[
                "cc_number",
                "change_count",
                "midi_channel",
                "start_value",
                "end_value",
            ],
            EventType::MidiEvent => &["midi_channel"],
            _ => &[],
        }
    }
}

fn field<'a>(event: &'a Map<String, Value>, key: &str) -> &'a Value {
    event.get(key).unwrap_or(&Value::Null)
}

fn as_known_str(value: &Value) -> Option<&str> {
    value.as_str()
}

/// `Ok(())` when the event is well-formed, else the Python validator's exact
/// rejection reason: it is echoed to the plug-in in the acknowledgement and, via
/// the receiver log, ends up quoted in operator reports.
pub fn validate_event(event: &Map<String, Value>) -> core::result::Result<EventType, String> {
    let raw_type = field(event, "event_type");
    let Some(event_type) = as_known_str(raw_type).and_then(EventType::parse) else {
        return Err(format!("Unknown event type: {}", python_str(raw_type)));
    };

    let raw_level = field(event, "proof_level");
    if as_known_str(raw_level)
        .and_then(|value| ProofLevel::parse(value).ok())
        .is_none()
    {
        return Err(format!("Unknown proof level: {}", python_str(raw_level)));
    }

    for required in event_type.required_fields() {
        if !event.contains_key(*required) {
            return Err(format!(
                "Missing required field '{required}' for {}",
                event_type.as_str()
            ));
        }
    }
    Ok(event_type)
}

/// [`validate_event`] plus the origin and proof-level caps for UDP-received
/// events.
pub fn validate_network_event(
    event: &Map<String, Value>,
) -> core::result::Result<EventType, String> {
    let event_type = validate_event(event)?;

    let Some(cap) = event_type.network_proof_level_cap() else {
        return Err(format!(
            "Event type not accepted from the network: {} (daemon-origin event types must not arrive over UDP)",
            event_type.as_str()
        ));
    };

    let proof_level = as_known_str(field(event, "proof_level"))
        .and_then(|value| ProofLevel::parse(value).ok())
        .unwrap_or(ProofLevel::UnknownUnobserved);
    if proof_level.rank() > cap.rank() {
        return Err(format!(
            "Proof level '{}' exceeds network cap '{}' for {}",
            proof_level.as_str(),
            cap.as_str(),
            event_type.as_str()
        ));
    }

    for numeric in event_type.network_numeric_fields() {
        let value = field(event, numeric);
        if !value.is_null() && !is_finite_number(value) {
            return Err(format!(
                "Field '{numeric}' must be a finite number for {}",
                event_type.as_str()
            ));
        }
    }

    if event_type == EventType::BufferHash {
        let envelope = field(event, "energy_envelope");
        let acceptable = match envelope {
            Value::Null => true,
            Value::Array(items) => items.iter().all(is_finite_number),
            _ => false,
        };
        if !acceptable {
            return Err("Field 'energy_envelope' must be a list of finite numbers".to_owned());
        }
    }

    Ok(event_type)
}

/// IMPORTANT: `serde_json` decodes no non-finite literal, so the finiteness half
/// of Python's `math.isfinite` guard is unreachable from a parsed datagram; the
/// check that bites is the type check. A JSON boolean is rejected here where
/// Python's `isinstance(True, int)` accepts it (see the crate's open issues).
fn is_finite_number(value: &Value) -> bool {
    match value {
        Value::Number(number) => number.as_f64().is_some_and(f64::is_finite),
        _ => false,
    }
}
