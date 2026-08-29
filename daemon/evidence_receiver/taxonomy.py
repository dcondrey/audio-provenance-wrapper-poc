from __future__ import annotations

from collections.abc import Mapping
from enum import Enum


class EventType(str, Enum):
    """Every observation event produced by the plugin or daemon."""

    BUFFER_HASH = "buffer_hash"
    AUDIO_TRANSITION = "audio_transition"
    SPECTRAL_SHIFT = "spectral_shift"
    TRANSPORT_CHANGE = "transport_change"
    MIDI_EVENT = "midi_event"
    SESSION_CONFIG = "session_config_change"
    SPECTRAL_PROFILE_CHANGE = "spectral_profile_change"
    PARAMETER_CHANGE = "parameter_change"
    SAMPLE_FILE_OBSERVED = "sample_file_observed"
    INGREDIENT_CORRELATION = "ingredient_correlation"
    COMPOSITE_EDIT = "composite_edit"
    FORGERY_ANALYSIS = "forgery_analysis"
    PROJECT_DIFF = "project_diff"
    PROJECT_SAVE_DETECTED = "project_save_detected"
    LAYER_UNAVAILABLE = "layer_unavailable"


class ProofLevel(str, Enum):
    """How confident the system is in a given claim."""

    DIRECTLY_OBSERVED = "directly_observed"
    INFERRED = "inferred"
    USER_DECLARED = "user_declared"
    EXTERNALLY_VERIFIED = "externally_verified"
    UNKNOWN_UNOBSERVED = "unknown_unobserved"


REQUIRED_FIELDS: dict[str, list[str]] = {
    EventType.BUFFER_HASH: [
        "window_hash",
        "prev_hash",
        "rms_level",
        "zero_crossing_rate",
    ],
    EventType.AUDIO_TRANSITION: ["direction", "boundary_hash"],
    EventType.SPECTRAL_SHIFT: [
        "prev_spectral_centroid_hz",
        "new_spectral_centroid_hz",
    ],
    EventType.TRANSPORT_CHANGE: ["transport_state"],
    EventType.MIDI_EVENT: ["midi_event_type", "midi_channel"],
    EventType.SESSION_CONFIG: ["sample_rate_hz", "channel_count"],
    EventType.SPECTRAL_PROFILE_CHANGE: [
        "band_low_delta",
        "band_mid_delta",
        "band_high_delta",
    ],
    EventType.PARAMETER_CHANGE: ["cc_number", "change_count"],
    EventType.SAMPLE_FILE_OBSERVED: ["sha256", "file_name"],
    EventType.INGREDIENT_CORRELATION: ["sample_sha256", "confidence"],
    EventType.COMPOSITE_EDIT: ["edit_type", "confidence", "contributing_events"],
    EventType.FORGERY_ANALYSIS: ["suspicion_score", "flags"],
    EventType.PROJECT_DIFF: ["clips_added", "clips_removed"],
    EventType.PROJECT_SAVE_DETECTED: ["file_hash"],
    EventType.LAYER_UNAVAILABLE: ["layer", "reason"],
}

_VALID_EVENT_TYPES = frozenset(e.value for e in EventType)
_VALID_PROOF_LEVELS = frozenset(p.value for p in ProofLevel)

# Evidence-strength order for cap comparisons only; not a general trust ranking.
_PROOF_LEVEL_RANK: dict[str, int] = {
    ProofLevel.UNKNOWN_UNOBSERVED: 0,
    ProofLevel.USER_DECLARED: 1,
    ProofLevel.INFERRED: 2,
    ProofLevel.DIRECTLY_OBSERVED: 3,
    ProofLevel.EXTERNALLY_VERIFIED: 4,
}

# IMPORTANT: honesty constraint 1 (labels never stronger than evidence). The UDP
# socket cannot authenticate its sender, so a network event may claim at most what
# the in-process plugin observer legitimately asserts for that event type. Event
# types produced only by daemon-side observers (sample watcher, correlators,
# analyzers, project differ) never travel over UDP and are rejected there.
NETWORK_PROOF_LEVEL_CAP: dict[str, str] = {
    EventType.BUFFER_HASH: ProofLevel.DIRECTLY_OBSERVED,
    EventType.AUDIO_TRANSITION: ProofLevel.DIRECTLY_OBSERVED,
    EventType.SPECTRAL_SHIFT: ProofLevel.DIRECTLY_OBSERVED,
    EventType.TRANSPORT_CHANGE: ProofLevel.DIRECTLY_OBSERVED,
    EventType.MIDI_EVENT: ProofLevel.DIRECTLY_OBSERVED,
    EventType.SESSION_CONFIG: ProofLevel.DIRECTLY_OBSERVED,
    EventType.SPECTRAL_PROFILE_CHANGE: ProofLevel.DIRECTLY_OBSERVED,
    EventType.PARAMETER_CHANGE: ProofLevel.DIRECTLY_OBSERVED,
}


def validate_event(event: Mapping[str, object]) -> tuple[bool, str]:
    """Return (True, '') if the event is well-formed, else (False, reason)."""
    event_type = event.get("event_type")
    if event_type not in _VALID_EVENT_TYPES:
        return False, f"Unknown event type: {event_type}"

    proof_level = event.get("proof_level")
    if proof_level not in _VALID_PROOF_LEVELS:
        return False, f"Unknown proof level: {proof_level}"

    required = REQUIRED_FIELDS.get(str(event_type), [])
    for field in required:
        if field not in event:
            return False, f"Missing required field '{field}' for {event_type}"

    return True, ""


def validate_network_event(event: Mapping[str, object]) -> tuple[bool, str]:
    """validate_event() plus the origin/proof-level caps for UDP-received events."""
    valid, error = validate_event(event)
    if not valid:
        return valid, error

    event_type = str(event.get("event_type"))
    cap = NETWORK_PROOF_LEVEL_CAP.get(event_type)
    if cap is None:
        return False, (
            f"Event type not accepted from the network: {event_type} "
            "(daemon-origin event types must not arrive over UDP)"
        )

    proof_level = str(event.get("proof_level"))
    if _PROOF_LEVEL_RANK[proof_level] > _PROOF_LEVEL_RANK[cap]:
        return False, (
            f"Proof level '{proof_level}' exceeds network cap '{cap}' for {event_type}"
        )

    return True, ""
