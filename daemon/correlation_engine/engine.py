from __future__ import annotations

import json
import logging
import threading
from collections import deque
from dataclasses import dataclass, field
from pathlib import Path

from daemon.common import append_jsonl

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class LayerEvent:
    """A single event from any observation layer."""

    layer: str
    event_type: str
    timestamp_ms: int
    data: dict[str, object]


@dataclass
class CorrelationCandidate:
    """A group of temporally-aligned events from multiple layers."""

    window_center_ms: int
    events: list[LayerEvent] = field(default_factory=list)

    @property
    def layers_present(self) -> set[str]:
        return {e.layer for e in self.events}

    @property
    def span_ms(self) -> int:
        if len(self.events) < 2:
            return 0
        timestamps = [e.timestamp_ms for e in self.events]
        return max(timestamps) - min(timestamps)


@dataclass
class CompositeEdit:
    """An edit event inferred from cross-layer correlation."""

    edit_type: str
    confidence: float
    timestamp_ms: int
    contributing_events: list[dict[str, object]]
    notes: list[str] = field(default_factory=list)

    def to_event_dict(self) -> dict[str, object]:
        return {
            "event_type": "composite_edit",
            "proof_level": "inferred",
            "timestamp_ms": self.timestamp_ms,
            "edit_type": self.edit_type,
            "confidence": round(self.confidence, 3),
            "contributing_events": self.contributing_events,
            "notes": self.notes,
        }


class CorrelationRule:
    """Base class for correlation rules.

    Each rule examines a CorrelationCandidate and optionally returns a
    CompositeEdit if the pattern matches.
    """

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        raise NotImplementedError


class ClipPasteRule(CorrelationRule):
    """Detect clip paste: Cmd+V followed by silence-to-audio transition.

    TODO: Cannot fire until daemon/input_capture/ CGEventTap is implemented.
    Requires input_capture layer to emit input_shortcut events with
    probable_operation="paste". See docs/MULTI_LAYER_OBSERVATION.md Phase 2.
    """

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_paste_shortcut = any(
            e.layer == "input_capture"
            and e.data.get("probable_operation") == "paste"
            for e in candidate.events
        )
        has_audio_start = any(
            e.layer == "audio_buffer"
            and e.event_type == "audio_transition"
            and e.data.get("direction") == "silence_to_audio"
            for e in candidate.events
        )
        if not (has_paste_shortcut and has_audio_start):
            return None

        confidence = 0.85
        confidence += _alignment_bonus(candidate)
        confidence += _layer_bonus(candidate, required=2)

        return CompositeEdit(
            edit_type="clip_paste",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                (e.layer == "input_capture" and e.data.get("probable_operation") == "paste")
                or (e.event_type == "audio_transition" and e.data.get("direction") == "silence_to_audio")
            )),
        )


class ClipDeleteRule(CorrelationRule):
    """Detect clip delete: Delete key followed by audio-to-silence transition.

    TODO: Cannot fire until daemon/input_capture/ CGEventTap is implemented.
    """

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_delete = any(
            e.layer == "input_capture"
            and e.data.get("probable_operation") == "delete"
            for e in candidate.events
        )
        has_audio_stop = any(
            e.layer == "audio_buffer"
            and e.event_type == "audio_transition"
            and e.data.get("direction") == "audio_to_silence"
            for e in candidate.events
        )
        if not (has_delete and has_audio_stop):
            return None

        confidence = 0.80
        confidence += _alignment_bonus(candidate)
        confidence += _layer_bonus(candidate, required=2)

        return CompositeEdit(
            edit_type="clip_delete",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                (e.layer == "input_capture" and e.data.get("probable_operation") == "delete")
                or (e.event_type == "audio_transition" and e.data.get("direction") == "audio_to_silence")
            )),
        )


class EffectChangeRule(CorrelationRule):
    """Detect effect change: mixer visual change + spectral shift, no silence transition.

    TODO: Cannot fire until daemon/screen_observer/ capture is implemented.
    """

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_mixer_change = any(
            e.layer == "screen_observer"
            and e.event_type == "screen_mixer_changed"
            for e in candidate.events
        )
        has_spectral_shift = any(
            e.layer == "audio_buffer"
            and e.event_type == "spectral_shift"
            for e in candidate.events
        )
        has_silence_transition = any(
            e.layer == "audio_buffer"
            and e.event_type == "audio_transition"
            for e in candidate.events
        )
        if not (has_mixer_change and has_spectral_shift and not has_silence_transition):
            return None

        confidence = 0.70
        confidence += _alignment_bonus(candidate)
        confidence += _layer_bonus(candidate, required=2)

        return CompositeEdit(
            edit_type="effect_change",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                e.event_type in {"screen_mixer_changed", "spectral_shift"}
            )),
        )


class SampleImportRule(CorrelationRule):
    """Detect confirmed sample import: file observed + project ref + stream correlation."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        sample_event = None
        for e in candidate.events:
            if e.layer == "sample_watcher" and e.event_type == "sample_file_observed":
                sample_event = e
                break

        has_project_ref = any(
            e.layer == "project_differ"
            and e.event_type == "project_sample_ref_added"
            for e in candidate.events
        )
        has_correlation = any(
            e.layer == "audio_buffer"
            and e.event_type == "ingredient_correlation"
            for e in candidate.events
        )

        if sample_event is None:
            return None

        confidence = 0.60
        if has_project_ref:
            confidence += 0.15
        if has_correlation:
            confidence += 0.15
        confidence += _layer_bonus(candidate, required=1)

        return CompositeEdit(
            edit_type="sample_import_confirmed",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                e is sample_event
                or e.event_type in {"project_sample_ref_added", "ingredient_correlation"}
            )),
            notes=[f"Sample: {sample_event.data.get('file_name', 'unknown')}"],
        )


class UndoRule(CorrelationRule):
    """Detect undo: Cmd+Z followed by content hash matching an earlier window.

    TODO: Cannot fire until daemon/input_capture/ CGEventTap is implemented.
    TODO: Hash chain rollback detection (matching current hash to earlier window)
    not yet implemented; would increase confidence significantly.
    """

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_undo = any(
            e.layer == "input_capture"
            and e.data.get("probable_operation") == "undo"
            for e in candidate.events
        )
        if not has_undo:
            return None

        confidence = 0.75
        confidence += _alignment_bonus(candidate)

        return CompositeEdit(
            edit_type="undo",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                e.layer == "input_capture" and e.data.get("probable_operation") == "undo"
            )),
            notes=["Hash chain rollback detection not yet implemented."],
        )


class ArrangementEditRule(CorrelationRule):
    """Detect arrangement edit: project diff + screen change + input activity.

    TODO: screen_observer and input_capture layers boost confidence but are
    not required. This rule fires with project_differ alone at base confidence.
    """

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_project_diff = any(
            e.layer == "project_differ"
            and e.event_type == "project_diff"
            for e in candidate.events
        )
        has_screen_change = any(
            e.layer == "screen_observer"
            and e.event_type == "screen_arrangement_changed"
            for e in candidate.events
        )
        has_input = any(
            e.layer == "input_capture"
            for e in candidate.events
        )

        if not has_project_diff:
            return None

        confidence = 0.65
        if has_screen_change:
            confidence += 0.05
        if has_input:
            confidence += 0.05
        confidence += _layer_bonus(candidate, required=1)

        diff_data = {}
        for e in candidate.events:
            if e.layer == "project_differ" and e.event_type == "project_diff":
                diff_data = e.data
                break

        return CompositeEdit(
            edit_type="arrangement_edit",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                e.event_type in {"project_diff", "screen_arrangement_changed"}
                or e.layer == "input_capture"
            )),
            notes=[
                f"clips +{diff_data.get('clips_added', 0)}"
                f"/-{diff_data.get('clips_removed', 0)}"
                f"/~{diff_data.get('clips_modified', 0)}"
            ],
        )


def _alignment_bonus(candidate: CorrelationCandidate) -> float:
    if candidate.span_ms < 200:
        return 0.05
    if candidate.span_ms > 1000:
        return -0.10
    return 0.0


def _layer_bonus(candidate: CorrelationCandidate, required: int) -> float:
    extra = len(candidate.layers_present) - required
    return max(0.0, extra * 0.05)


def _summarize_events(
    candidate: CorrelationCandidate,
    predicate=None,
) -> list[dict[str, object]]:
    events = candidate.events if predicate is None else [e for e in candidate.events if predicate(e)]
    return [
        {
            "layer": e.layer,
            "event_type": e.event_type,
            "timestamp_ms": e.timestamp_ms,
            "source_timestamp_ms": e.data.get("source_timestamp_ms", e.data.get("timestamp_ms")),
            "event_id": e.data.get(
                "daemon_event_id",
                f"{e.layer}:{e.event_type}:{e.timestamp_ms}:{e.data.get('event_sequence', '')}",
            ),
        }
        for e in events
    ]


class ParameterAdjustRule(CorrelationRule):
    """Detect knob/fader adjustment: parameter_change + spectral_profile_change."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_param = any(
            e.layer == "midi"
            and e.event_type == "parameter_change"
            for e in candidate.events
        )
        has_profile = any(
            e.layer == "audio_buffer"
            and e.event_type == "spectral_profile_change"
            for e in candidate.events
        )
        if not (has_param and has_profile):
            return None

        param_event = next(
            (e for e in candidate.events if e.event_type == "parameter_change"), None
        )
        cc = param_event.data.get("cc_number", "?") if param_event else "?"

        confidence = 0.80
        confidence += _alignment_bonus(candidate)
        confidence += _layer_bonus(candidate, required=2)

        return CompositeEdit(
            edit_type="effect_adjusted",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                e.event_type in {"parameter_change", "spectral_profile_change"}
            )),
            notes=[f"CC#{cc} change correlated with spectral profile shift"],
        )


class RecordingStartRule(CorrelationRule):
    """Detect recording: transport_change(playing/recording) + audio_transition."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_transport_play = any(
            e.layer == "transport"
            and e.event_type == "transport_change"
            and e.data.get("transport_state") in ("playing", "recording")
            for e in candidate.events
        )
        has_audio_start = any(
            e.layer == "audio_buffer"
            and e.event_type == "audio_transition"
            and e.data.get("direction") == "silence_to_audio"
            for e in candidate.events
        )
        if not (has_transport_play and has_audio_start):
            return None

        is_recording = any(
            e.data.get("transport_state") == "recording"
            for e in candidate.events
            if e.event_type == "transport_change"
        )
        edit_type = "recording_started" if is_recording else "playback_started"

        confidence = 0.85
        confidence += _alignment_bonus(candidate)

        return CompositeEdit(
            edit_type=edit_type,
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                (e.event_type == "transport_change" and e.data.get("transport_state") in ("playing", "recording"))
                or (e.event_type == "audio_transition" and e.data.get("direction") == "silence_to_audio")
            )),
        )


class ContentChangeRule(CorrelationRule):
    """Detect content change: spectral_shift + audio continues (no silence)."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        has_spectral_shift = any(
            e.layer == "audio_buffer"
            and e.event_type == "spectral_shift"
            for e in candidate.events
        )
        has_transition = any(
            e.layer == "audio_buffer"
            and e.event_type == "audio_transition"
            for e in candidate.events
        )
        if not has_spectral_shift or has_transition:
            return None

        has_profile_change = any(
            e.event_type == "spectral_profile_change"
            for e in candidate.events
        )

        confidence = 0.55
        if has_profile_change:
            confidence += 0.10
        confidence += _layer_bonus(candidate, required=1)

        return CompositeEdit(
            edit_type="content_changed",
            confidence=min(confidence, 1.0),
            timestamp_ms=candidate.window_center_ms,
            contributing_events=_summarize_events(candidate, lambda e: (
                e.event_type in {"spectral_shift", "spectral_profile_change"}
            )),
        )


class DeviceAddedRule(CorrelationRule):
    """Detect plugin/device added: project_diff with devices_changed."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        for e in candidate.events:
            if e.layer != "project_differ" or e.event_type != "project_diff":
                continue
            devices = e.data.get("devices_changed", [])
            if not devices:
                continue

            confidence = 0.70
            confidence += _layer_bonus(candidate, required=1)

            return CompositeEdit(
                edit_type="device_chain_changed",
                confidence=min(confidence, 1.0),
                timestamp_ms=candidate.window_center_ms,
                contributing_events=_summarize_events(candidate, lambda item: item is e),
                notes=[f"Devices changed on: {', '.join(str(d) for d in devices[:5])}"],
            )
        return None


class AutomationEditRule(CorrelationRule):
    """Detect automation editing: project_diff with automation_points_delta."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        for e in candidate.events:
            if e.layer != "project_differ" or e.event_type != "project_diff":
                continue
            delta = e.data.get("automation_points_delta", 0)
            if delta == 0:
                continue

            confidence = 0.65
            confidence += _layer_bonus(candidate, required=1)

            return CompositeEdit(
                edit_type="automation_edited",
                confidence=min(confidence, 1.0),
                timestamp_ms=candidate.window_center_ms,
                contributing_events=_summarize_events(candidate, lambda item: item is e),
                notes=[f"Automation points delta: {delta:+d}"],
            )
        return None


class MidiEditRule(CorrelationRule):
    """Detect MIDI editing: project_diff with midi_notes_delta."""

    def evaluate(self, candidate: CorrelationCandidate) -> CompositeEdit | None:
        for e in candidate.events:
            if e.layer != "project_differ" or e.event_type != "project_diff":
                continue
            delta = e.data.get("midi_notes_delta", 0)
            if delta == 0:
                continue

            confidence = 0.65
            confidence += _layer_bonus(candidate, required=1)

            return CompositeEdit(
                edit_type="midi_edited",
                confidence=min(confidence, 1.0),
                timestamp_ms=candidate.window_center_ms,
                contributing_events=_summarize_events(candidate, lambda item: item is e),
                notes=[f"MIDI notes delta: {delta:+d}"],
            )
        return None


DEFAULT_RULES: list[CorrelationRule] = [
    ClipPasteRule(),
    ClipDeleteRule(),
    EffectChangeRule(),
    ParameterAdjustRule(),
    RecordingStartRule(),
    ContentChangeRule(),
    DeviceAddedRule(),
    AutomationEditRule(),
    MidiEditRule(),
    SampleImportRule(),
    UndoRule(),
    ArrangementEditRule(),
]


class CorrelationEngine:
    """Consumes events from all layers and emits composite edit evidence.

    Events are buffered in a sliding time window. When the window advances,
    temporally-aligned groups are evaluated against correlation rules.
    """

    def __init__(
        self,
        window_ms: int = 2000,
        rules: list[CorrelationRule] | None = None,
        evidence_path: Path = Path("evidence/composite_events.jsonl"),
        max_buffer_events: int = 512,
        max_dedup_keys: int = 4096,
    ) -> None:
        self.window_ms = window_ms
        self.rules = rules or list(DEFAULT_RULES)
        self.evidence_path = evidence_path.expanduser()
        self._buffer: deque[LayerEvent] = deque()
        self._emitted_count = 0
        self.max_buffer_events = max(2, max_buffer_events)
        self.max_dedup_keys = max(32, max_dedup_keys)
        self._dedup_keys: set[str] = set()
        self._dedup_order: deque[str] = deque()
        self._capacity_drops = 0
        self._duplicate_suppressions = 0
        # IMPORTANT: ingest() is called from the udp-receiver, sample-watcher, and
        # project-watcher threads; buffer/dedup state must mutate atomically, and
        # append_jsonl's check-then-rotate is not safe under concurrent writers.
        self._lock = threading.Lock()
        self._write_lock = threading.Lock()

    def ingest(self, event: LayerEvent) -> list[CompositeEdit]:
        """Add an event and return any composite edits triggered."""
        with self._lock:
            self._buffer.append(event)
            self._expire_old_events(event.timestamp_ms)
            while len(self._buffer) > self.max_buffer_events:
                self._buffer.popleft()
                self._capacity_drops += 1
                if self._capacity_drops == 1 or self._capacity_drops % 1000 == 0:
                    log.warning(
                        "Correlation buffer capacity reached; dropped=%d max_events=%d",
                        self._capacity_drops,
                        self.max_buffer_events,
                    )
            results = self._evaluate()
        for composite in results:
            try:
                with self._write_lock:
                    self._write_event(composite)
            except OSError:
                log.exception("Could not append composite edit evidence")
        return results

    def _expire_old_events(self, now_ms: int) -> None:
        cutoff = now_ms - self.window_ms
        while self._buffer and self._buffer[0].timestamp_ms < cutoff:
            self._buffer.popleft()

    def _evaluate(self) -> list[CompositeEdit]:
        if len(self._buffer) < 2:
            return []

        candidate = CorrelationCandidate(
            window_center_ms=self._buffer[-1].timestamp_ms,
            events=list(self._buffer),
        )

        results: list[CompositeEdit] = []
        for rule in self.rules:
            composite = rule.evaluate(candidate)
            if composite is not None and composite.confidence >= 0.5:
                if not composite.contributing_events:
                    continue
                dedup_key = json.dumps(
                    [
                        composite.edit_type,
                        sorted(
                            str(item.get("event_id", ""))
                            for item in composite.contributing_events
                        ),
                    ],
                    separators=(",", ":"),
                )
                if dedup_key in self._dedup_keys:
                    self._duplicate_suppressions += 1
                    continue
                self._dedup_keys.add(dedup_key)
                self._dedup_order.append(dedup_key)
                while len(self._dedup_order) > self.max_dedup_keys:
                    expired = self._dedup_order.popleft()
                    self._dedup_keys.discard(expired)
                results.append(composite)
                self._emitted_count += 1

        return results

    def _write_event(self, composite: CompositeEdit) -> None:
        append_jsonl(self.evidence_path, composite.to_event_dict())

    @property
    def emitted_count(self) -> int:
        return self._emitted_count

    @property
    def buffer_size(self) -> int:
        return len(self._buffer)

    @property
    def capacity_drops(self) -> int:
        return self._capacity_drops

    @property
    def duplicate_suppressions(self) -> int:
        return self._duplicate_suppressions
