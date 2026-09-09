from __future__ import annotations

import math
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
    HOST_ENVIRONMENT = "host_environment"


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
    EventType.HOST_ENVIRONMENT: ["host_recognised", "wrapper_format"],
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
    EventType.HOST_ENVIRONMENT: ProofLevel.DIRECTLY_OBSERVED,
}

# IMPORTANT: downstream consumers (status readiness, audio association, sample
# correlation) do arithmetic on these fields straight off the wire; a non-numeric
# value crashes those threads, so the type invariant is established here at the
# UDP boundary rather than re-checked at every consumer.
NETWORK_NUMERIC_FIELDS: dict[str, tuple[str, ...]] = {
    EventType.BUFFER_HASH: (
        "rms_level", "zero_crossing_rate", "window_size_samples",
        "sample_rate_hz", "spectral_centroid_hz", "crest_factor",
        # Read by generator.py as int(); omitting them let one datagram sent
        # before the plug-in's first window abort every export in the session.
        "timestamp_ms", "channel_count",
    ),
    EventType.SPECTRAL_SHIFT: ("prev_spectral_centroid_hz", "new_spectral_centroid_hz"),
    EventType.SESSION_CONFIG: ("sample_rate_hz", "channel_count"),
    EventType.SPECTRAL_PROFILE_CHANGE: ("band_low_delta", "band_mid_delta", "band_high_delta"),
    EventType.PARAMETER_CHANGE: (
        "cc_number", "change_count", "midi_channel", "start_value", "end_value",
    ),
    EventType.MIDI_EVENT: ("midi_channel",),
}

# IMPORTANT: honesty constraint 1 for free text. These fields name the host
# application observed around the plug-in and are rendered into the signed
# manifest, so an unauthenticated socket must not be able to write an unbounded
# or non-string value into a signed claim.
MAX_TEXT_FIELD_CHARS = 128
NETWORK_TEXT_FIELDS: dict[str, tuple[str, ...]] = {
    EventType.HOST_ENVIRONMENT: ("host_name", "host_executable_name", "wrapper_format"),
}
NETWORK_BOOLEAN_FIELDS: dict[str, tuple[str, ...]] = {
    EventType.HOST_ENVIRONMENT: ("host_recognised",),
}


# IMPORTANT: honesty constraint 1 again, for the nested telemetry map. These
# counters are copied into the daemon's coverage map and signed into the
# manifest as observation coverage, so an unvalidated value from an
# unauthenticated socket writes directly into a signed claim.
MAX_TELEMETRY_ENTRIES = 32
MAX_TELEMETRY_KEY_CHARS = 64
MAX_COUNTER_VALUE = 2**53
MAX_STREAM_KEY_CHARS = 128
# json.loads accepts arbitrarily deep structures; the evidence writer refuses
# them, so the boundary must too or stream state advances for an event that is
# never persisted.
MAX_EVENT_DEPTH = 8


def _validate_telemetry(telemetry: object) -> str:
    if not isinstance(telemetry, Mapping):
        return "Field 'telemetry' must be an object of cumulative integer counters"
    if len(telemetry) > MAX_TELEMETRY_ENTRIES:
        return f"Field 'telemetry' carries more than {MAX_TELEMETRY_ENTRIES} counters"
    for key, value in telemetry.items():
        if not isinstance(key, str) or not key or len(key) > MAX_TELEMETRY_KEY_CHARS:
            return "Telemetry counter names must be non-empty strings of at most 64 characters"
        if isinstance(value, bool) or not isinstance(value, int):
            return f"Telemetry counter {key!r} must be an integer"
        if not 0 <= value <= MAX_COUNTER_VALUE:
            return f"Telemetry counter {key!r} is outside the plausible cumulative range"
    return ""


def _is_serialisable(value: object, depth: int = 0) -> bool:
    """Reject anything the evidence writer would refuse after state has moved.

    Validation was a per-field whitelist while serialization is whole-object with
    allow_nan=False, so a NaN in any unlisted field advanced the chain head and
    then raised inside the evidence write.
    """
    if depth > MAX_EVENT_DEPTH:
        return False
    if isinstance(value, bool):
        return True
    if isinstance(value, float):
        return math.isfinite(value)
    if isinstance(value, Mapping):
        return all(
            isinstance(key, str) and _is_serialisable(child, depth + 1)
            for key, child in value.items()
        )
    if isinstance(value, (list, tuple)):
        return all(_is_serialisable(child, depth + 1) for child in value)
    return True


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

    if not _is_serialisable(event):
        return False, (
            "Event contains a non-finite number, a non-string key, or exceeds "
            f"{MAX_EVENT_DEPTH} levels of nesting"
        )

    for key in ("plugin_instance_id", "plugin_capture_session_id"):
        value = event.get(key)
        if value is not None and (
            not isinstance(value, str) or len(value) > MAX_STREAM_KEY_CHARS
        ):
            return False, f"Field '{key}' must be a string of at most {MAX_STREAM_KEY_CHARS} characters"

    telemetry = event.get("telemetry")
    if telemetry is not None:
        error = _validate_telemetry(telemetry)
        if error:
            return False, error

    for field in NETWORK_NUMERIC_FIELDS.get(event_type, ()):
        value = event.get(field)
        if value is not None and not _is_finite_number(value):
            return False, f"Field '{field}' must be a finite number for {event_type}"
    required = REQUIRED_FIELDS.get(event_type, [])
    for field in NETWORK_TEXT_FIELDS.get(event_type, ()):
        value = event.get(field)
        # A null is honest for an optional field (an unrecognised host has no
        # name) but never for one the event type declares as required.
        if value is None and field not in required:
            continue
        if not isinstance(value, str) or len(value) > MAX_TEXT_FIELD_CHARS:
            return False, (
                f"Field '{field}' must be a string of at most "
                f"{MAX_TEXT_FIELD_CHARS} characters for {event_type}"
            )

    for field in NETWORK_BOOLEAN_FIELDS.get(event_type, ()):
        if not isinstance(event.get(field), bool):
            return False, f"Field '{field}' must be a boolean for {event_type}"

    if event_type == EventType.BUFFER_HASH:
        envelope = event.get("energy_envelope")
        if envelope is not None and (
            not isinstance(envelope, list)
            or not all(_is_finite_number(v) for v in envelope)
        ):
            return False, "Field 'energy_envelope' must be a list of finite numbers"

    return True, ""


def _is_finite_number(value: object) -> bool:
    # json.loads accepts the non-standard NaN/Infinity literals, and int(nan)
    # raises ValueError downstream, so isinstance alone does not close the hole.
    return isinstance(value, (int, float)) and math.isfinite(value)
