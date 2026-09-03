from __future__ import annotations

import json
import logging
from dataclasses import dataclass, field
from pathlib import Path

from daemon.common import APW_VERSION, utc_timestamp

log = logging.getLogger(__name__)

C2PA_CLAIM_SCOPE = (
    "A real C2PA claim signed with a locally issued X.509 chain and bound to the "
    "asset by a SHA-256 hard binding. The signing act and the binding are directly "
    "observed. The signer identity is self-asserted: a 'verified' validation state "
    "means the claim chains to this machine's own root certificate, not to any "
    "external trust list, registry, or verified creator identity."
)


def unavailable_c2pa_claim(reason: str) -> dict[str, object]:
    return {
        "status": "unavailable",
        "reason": reason,
        "scope": C2PA_CLAIM_SCOPE,
        "apw:proof_level": "unknown_unobserved",
    }


@dataclass(frozen=True)
class StemEvidence:
    """Evidence for a single observed audio stem."""

    stem_id: str
    hash_chain_root: str
    hash_chain_length: int
    first_observed_ms: int
    last_observed_ms: int
    sample_rate_hz: int
    channel_count: int
    source_category: str
    proof_level: str
    source_category_proof_level: str = "unknown_unobserved"
    hash_chain_genesis: str = "genesis"
    first_received_at: str | None = None
    last_received_at: str | None = None
    plugin_instance_ids: tuple[str, ...] = ()


@dataclass(frozen=True)
class ExportEvidence:
    """Evidence for a final exported audio file."""

    file_path: str
    file_name: str
    sha256: str
    format: str
    file_size_bytes: int
    duration_seconds: float | None
    exported_at: str
    sample_rate_hz: int | float | None = None
    channel_count: int | None = None
    export_version: int = 1


@dataclass(frozen=True)
class IngredientEvidence:
    """Evidence for an observed sample ingredient."""

    file_name: str
    sha256: str
    proof_level: str
    correlation_confidence: float | None
    audio_fingerprint: dict[str, float | None] | None


@dataclass
class ManifestBuilder:
    """Builds a C2PA-compatible crJSON manifest from composite evidence.

    The manifest maps internal provenance evidence to C2PA assertion
    structures. This is the "fight card" manifest described in
    docs/MANIFEST_SCHEMA.md.

    Two C2PA views are emitted and they are not interchangeable:

    ``c2pa_mapping`` is a descriptive JSON projection of the internal evidence.
    It is not signed, not embedded, and not conformant; downstream code that
    reads assertion labels out of the manifest still reads this.

    ``c2pa_claim`` records the real claim produced by daemon.c2pa_engine and
    signed into (or beside) the asset: its validation state, hard binding, and
    signer. The generator supplies it; a builder used without one emits an
    honest unavailable record.

    c2pa_mapping projection:
        stems         → c2pa.ingredient assertions (type: audio/*)
        export        → c2pa.asset assertion (the final output)
        edit_history  → c2pa.actions assertion (edit operations)
        hash_chain    → c2pa.hash assertion (content binding)
        ingredients   → c2pa.ingredient assertions (samples)
        hardware      → c2pa.claim_signature (device binding)
        time_anchors  → c2pa.timestamp assertions

    Proof levels are preserved as custom extensions:
        "apw:proof_level": "directly_observed" | "inferred" | etc.

    Unknown or unobserved data is explicitly represented:
        "apw:unobserved": ["hidden_plugin_state", "bypassed_routing", ...]
    """

    session_id: str = ""
    created_at: str = field(default_factory=utc_timestamp)
    stems: list[StemEvidence] = field(default_factory=list)
    export: ExportEvidence | None = None
    ingredients: list[IngredientEvidence] = field(default_factory=list)
    composite_edits: list[dict[str, object]] = field(default_factory=list)
    hardware_binding: dict[str, object] | None = None
    time_anchors: list[dict[str, object]] = field(default_factory=list)
    forgery_report: dict[str, object] | None = None
    coverage: dict[str, object] | None = None
    c2pa_claim: dict[str, object] | None = None
    audio_association: dict[str, object] | None = None
    session_diagnostics: dict[str, object] | None = None
    unobserved: list[str] = field(default_factory=lambda: [
        "hidden_plugin_state",
        "internal_preset_logic",
        "bypassed_routing",
        "daw_internal_processing",
        "unverifiable_upstream_provenance",
    ])

    def add_stem(self, stem: StemEvidence) -> None:
        self.stems.append(stem)

    def set_export(self, export: ExportEvidence) -> None:
        self.export = export

    def add_ingredient(self, ingredient: IngredientEvidence) -> None:
        self.ingredients.append(ingredient)

    def add_composite_edit(self, edit: dict[str, object]) -> None:
        self.composite_edits.append(edit)

    def set_hardware_binding(self, binding: dict[str, object]) -> None:
        self.hardware_binding = binding

    def add_time_anchor(self, anchor: dict[str, object]) -> None:
        self.time_anchors.append(anchor)

    def set_forgery_report(self, report: dict[str, object]) -> None:
        self.forgery_report = report

    def set_c2pa_claim(self, claim: dict[str, object]) -> None:
        self.c2pa_claim = claim

    def build(self) -> dict[str, object]:
        """Build the complete crJSON manifest."""
        manifest: dict[str, object] = {
            "apw_version": APW_VERSION,
            "schema": "audio-provenance-manifest-v0",
            "session_id": self.session_id,
            "capture_session": {
                "id": self.session_id,
                "scope": "local_daemon_runtime",
                "apw:proof_level": "directly_observed",
            },
            "created_at": self.created_at,
            "core_principle": "Never claim full DAW provenance.",
            "daemon_receipt_acknowledgement": {
                "protocol": "apw-local-udp-ack-v1",
                "status": "unknown",
                "streams": [],
                "counters": {"attempted": 0, "sent": 0, "failed": 0},
                "scope": "No daemon acknowledgement evidence was supplied to the builder.",
                "apw:proof_level": "unknown_unobserved",
            },
        }

        manifest["observed_stems"] = [
            {
                "stem_id": s.stem_id,
                "hash_chain_root": s.hash_chain_root,
                "hash_chain_genesis": s.hash_chain_genesis,
                "hash_chain_length": s.hash_chain_length,
                "first_observed_ms": s.first_observed_ms,
                "last_observed_ms": s.last_observed_ms,
                "first_received_at": s.first_received_at,
                "last_received_at": s.last_received_at,
                "sample_rate_hz": s.sample_rate_hz,
                "channel_count": s.channel_count,
                "source_category": s.source_category,
                "source_category_proof_level": s.source_category_proof_level,
                "source": {
                    "category": s.source_category,
                    "apw:proof_level": s.source_category_proof_level,
                },
                "plugin_instance_ids": list(s.plugin_instance_ids),
                "apw:proof_level": s.proof_level,
            }
            for s in self.stems
        ]

        if self.export is not None:
            manifest["export"] = {
                "file_path": self.export.file_path,
                "file_name": self.export.file_name,
                "sha256": self.export.sha256,
                "format": self.export.format,
                "file_size_bytes": self.export.file_size_bytes,
                "duration_seconds": self.export.duration_seconds,
                "sample_rate_hz": self.export.sample_rate_hz,
                "channel_count": self.export.channel_count,
                "exported_at": self.export.exported_at,
                "export_version": self.export.export_version,
                "apw:proof_level": "directly_observed",
            }

        if self.ingredients:
            manifest["ingredients"] = [
                {
                    "file_name": i.file_name,
                    "sha256": i.sha256,
                    "apw:proof_level": i.proof_level,
                    "correlation_confidence": i.correlation_confidence,
                    "audio_fingerprint": i.audio_fingerprint,
                }
                for i in self.ingredients
            ]

        if self.composite_edits:
            manifest["edit_history"] = self.composite_edits

        if self.hardware_binding is not None:
            manifest["hardware_binding"] = self.hardware_binding

        if self.time_anchors:
            manifest["time_anchors"] = self.time_anchors

        if self.forgery_report is not None:
            manifest["forgery_analysis"] = self.forgery_report

        manifest["apw:unobserved"] = self.unobserved

        manifest["observation_coverage"] = self.coverage or {
            "status": "unknown_coverage",
            "basis": "No complete observation counters were available.",
            "counters": {},
            "apw:proof_level": "unknown_unobserved",
        }
        if self.session_diagnostics is not None:
            manifest["session_diagnostics"] = self.session_diagnostics

        default_association = {
            "status": "not_established",
            "capture_session_id": self.session_id,
            "stem_ids": [stem.stem_id for stem in self.stems],
            "export_file_name": self.export.file_name if self.export is not None else None,
            "basis": (
                "No routed-feature/export comparison was supplied. Session co-occurrence "
                "alone does not establish an audio association."
            ),
            "reason": "routed-feature comparison unavailable",
            "apw:proof_level": "unknown_unobserved",
        }
        manifest["stem_export_association"] = self.audio_association or default_association

        manifest["claim_summary"] = self._build_claim_summary()

        manifest["c2pa_mapping"] = {
            "status": "descriptive_projection_see_c2pa_claim_for_the_signed_claim",
            "claim_generator": f"AudioProvenanceCapture/{APW_VERSION}",
            "assertions": self._build_c2pa_assertions(),
        }
        manifest["c2pa_claim"] = self.c2pa_claim or unavailable_c2pa_claim(
            "No C2PA claim was supplied to the builder."
        )

        return manifest

    def _build_claim_summary(self) -> list[dict[str, object]]:
        """Return a concise, proof-labelled fight card for human review."""
        has_stem = bool(self.stems)
        has_export = self.export is not None
        association_record = self.audio_association or {}
        association = association_record.get("status") == "inferred_match"
        source = self.stems[0] if self.stems else None
        claim_record = self.c2pa_claim or {}
        c2pa_status = str(claim_record.get("status", "unavailable"))
        c2pa_signed = c2pa_status in {"embedded", "sidecar"}
        validation = claim_record.get("validation")
        if c2pa_signed and isinstance(validation, dict):
            c2pa_evidence = (
                f"{c2pa_status} claim, validation state {validation.get('state')}; "
                "trust evaluated against this machine's own root only"
            )
        else:
            c2pa_evidence = str(claim_record.get("reason", "No C2PA claim was produced"))

        return [
            {
                "claim": "observation_coverage",
                "value": (self.coverage or {}).get("status", "unknown_coverage"),
                "evidence": (self.coverage or {}).get(
                    "basis", "Complete counters were not available"
                ),
                "apw:proof_level": (self.coverage or {}).get(
                    "apw:proof_level", "unknown_unobserved"
                ),
            },
            {
                "claim": "routed_audio_observed",
                "value": has_stem,
                "evidence": (
                    f"{sum(stem.hash_chain_length for stem in self.stems)} hash windows received"
                    if has_stem
                    else "No buffer_hash events were received"
                ),
                "apw:proof_level": "directly_observed" if has_stem else "unknown_unobserved",
            },
            {
                "claim": "export_file_hashed",
                "value": has_export,
                "evidence": self.export.sha256 if self.export is not None else "No export detected",
                "apw:proof_level": "directly_observed" if has_export else "unknown_unobserved",
            },
            {
                "claim": "observed_stem_linked_to_export",
                "value": association,
                "evidence": _association_evidence(association_record),
                "apw:proof_level": (
                    "inferred" if association else "unknown_unobserved"
                ),
            },
            {
                "claim": "source_category",
                "value": source.source_category if source is not None else "unknown",
                "evidence": "producer declaration" if source is not None else "no observed stem",
                "apw:proof_level": (
                    source.source_category_proof_level if source is not None else "unknown_unobserved"
                ),
            },
            {
                "claim": "full_ableton_provenance",
                "value": False,
                "evidence": "Only audio routed through the capture plugin was observed",
                "apw:proof_level": "unknown_unobserved",
            },
            {
                "claim": "embedded_c2pa_claim",
                "value": c2pa_status,
                "evidence": c2pa_evidence,
                "apw:proof_level": (
                    "directly_observed" if c2pa_signed else "unknown_unobserved"
                ),
            },
        ]

    def _build_c2pa_assertions(self) -> list[dict[str, object]]:
        """Map internal evidence to C2PA assertion structures.

        C2PA assertion types used:
            c2pa.hash.data     - content hash binding
            c2pa.ingredient    - sample/stem references
            c2pa.actions       - edit actions performed
            c2pa.asset         - the final output file

        Custom assertions (apw: namespace):
            apw.proof_level    - evidence confidence classification
            apw.hash_chain     - rolling hash chain summary
            apw.unobserved     - explicitly unknown/unobservable data
        """
        assertions: list[dict[str, object]] = []

        if self.export is not None:
            assertions.append({
                "label": "c2pa.hash.data",
                "data": {
                    "name": self.export.file_name,
                    "hash": self.export.sha256,
                    "algorithm": "sha256",
                },
            })

        for stem in self.stems:
            assertions.append({
                "label": "c2pa.ingredient",
                "data": {
                    "title": stem.stem_id,
                    "relationship": "componentOf",
                    "apw:hash_chain_root": stem.hash_chain_root,
                    "apw:proof_level": stem.proof_level,
                },
            })

        for ingredient in self.ingredients:
            assertions.append({
                "label": "c2pa.ingredient",
                "data": {
                    "title": ingredient.file_name,
                    "relationship": "inputTo",
                    "hash": ingredient.sha256,
                    "apw:proof_level": ingredient.proof_level,
                },
            })

        if self.composite_edits:
            actions = []
            for edit in self.composite_edits:
                actions.append({
                    "action": _c2pa_action_type(str(edit.get("edit_type", "unknown"))),
                    "when": edit.get("timestamp_ms"),
                    "apw:edit_type": edit.get("edit_type"),
                    "apw:confidence": edit.get("confidence"),
                    "apw:proof_level": "inferred",
                })
            assertions.append({
                "label": "c2pa.actions",
                "data": {"actions": actions},
            })

        assertions.append({
            "label": "apw.unobserved",
            "data": {"items": self.unobserved},
        })

        return assertions

    def write_json(self, path: Path) -> None:
        path = path.expanduser()
        path.parent.mkdir(parents=True, exist_ok=True)
        manifest = self.build()
        with path.open("w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=2, ensure_ascii=False)
            f.write("\n")
        log.info("Manifest written to %s", path)


def _measurement(value: object) -> str:
    """Render a measurement, or say it was not measured. Never str(None)."""
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return f"{value:.4f}" if isinstance(value, float) else str(value)
    return "not measured"


def _association_evidence(record: dict[str, object]) -> str:
    """Evidence line for the stem-to-export claim card.

    IMPORTANT: this is signed and rendered verbatim on the fight card the founder
    demo closes on, so an unavailable comparison reports its cause rather than
    interpolating Python's None into the record.
    """
    if not record:
        return "No routed-feature comparison was supplied."
    method = str(record.get("method") or "no method recorded")
    if record.get("status") == "unavailable":
        return f"{method}: not measured ({record.get('reason', 'no reason recorded')})"
    return (
        f"{method}: confidence {_measurement(record.get('confidence'))} / "
        f"coverage {_measurement(record.get('matched_coverage'))}"
    )


def _c2pa_action_type(edit_type: str) -> str:
    """Map internal edit types to C2PA action vocabulary.

    C2PA action types from the specification:
        c2pa.created, c2pa.edited, c2pa.published, c2pa.opened,
        c2pa.placed, c2pa.removed, c2pa.unknown
    """
    mapping = {
        "clip_paste": "c2pa.placed",
        "clip_delete": "c2pa.removed",
        "effect_change": "c2pa.edited",
        "sample_import_confirmed": "c2pa.placed",
        "arrangement_edit": "c2pa.edited",
        "undo": "c2pa.edited",
    }
    return mapping.get(edit_type, "c2pa.unknown")
