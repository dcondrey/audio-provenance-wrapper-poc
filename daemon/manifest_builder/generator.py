"""Manifest assembly for a completed export: fight-card manifest, forgery
analysis, coverage derivation, signing, and companion artifact rendering.

Extracted from ``daemon.Daemon._generate_manifest`` / ``_derive_coverage`` /
``_derive_forgery_analysis``; these are thin wrappers around the ``Daemon``
instance rather than pure functions because the source method reads and
writes a large slice of daemon session state (session lock, event buffers,
last-artifact-path bookkeeping) that is not worth re-threading through an
explicit parameter list for a pure refactor.
"""

from __future__ import annotations

import dataclasses
import hashlib
import json
import logging
import os
import subprocess
import time
from pathlib import Path
from typing import TYPE_CHECKING

from daemon.audio_association import associate_export
from daemon.bundle import create_evidence_bundle
from daemon.common import sha256_file, sha256_prefix
from daemon.forgery_analysis.analyzer import (
    AudioStreamAnalyzer,
    ForgeryReport,
    HashChainAnalyzer,
    InputBehaviorAnalyzer,
)
from daemon.hardware_attestation.provider import SoftwareProvider
from daemon.manifest_builder.builder import (
    ExportEvidence,
    IngredientEvidence,
    ManifestBuilder,
    StemEvidence,
    C2PA_CLAIM_SCOPE,
    unavailable_c2pa_claim,
)
from daemon.report import write_html_report
from daemon.sample_watcher.watcher import extract_audio_metadata

if TYPE_CHECKING:
    from daemon.__main__ import Daemon

log = logging.getLogger(__name__)

_MAX_PROJECT_INGREDIENTS = 32
_MAX_INGREDIENT_HASH_BYTES = 256 * 1024 * 1024

_STEM_DIGEST_MEANING = (
    "rolling hash-chain root over the observed routed windows, not a file digest"
)
_OBSERVED_SAMPLE_DIGEST_MEANING = "sha-256 of the sample file as the daemon observed it"
_ASSOCIATION_ESTABLISHED = "inferred_match"

_UNOBSERVED_EVIDENCE: dict[str, str] = {
    "hidden_plugin_state": "Plug-in internal state is not exposed to a hosted audio effect.",
    "internal_preset_logic": "Device preset logic is not readable from the capture path.",
    "bypassed_routing": "Audio that did not pass through the capture plug-in was never seen.",
    "daw_internal_processing": "Processing inside the host, before or after the plug-in, was never seen.",
    "unverifiable_upstream_provenance": "No provenance was established for material created before this session.",
}
_UNOBSERVED_SAMPLE_DIGEST_MEANING = (
    "sha-256 read from disk at manifest time; the daemon never observed this file "
    "being imported, so it has no provenance of its own"
)


def derive_coverage(daemon: "Daemon", chain_length: int) -> dict[str, object]:
    receiver = daemon.receiver.diagnostics()
    with daemon._session_lock:
        telemetry = dict(daemon._latest_plugin_telemetry)
        plugin_instance_count = len(daemon._plugin_instance_ids)
        telemetry_regressions = daemon._telemetry_regressions
    counters: dict[str, int] = {
        **telemetry,
        **receiver,
        "plugin_telemetry_regressions": telemetry_regressions,
        "udp_sends_locally_emitted": max(
            0,
            telemetry.get("udp_sends_attempted", 0) - telemetry.get("udp_sends_failed", 0),
        ),
        "buffer_hash_events_received": chain_length,
        "feature_windows_dropped_from_alignment_buffer": daemon._feature_window_drops,
    }
    windows_hashed = telemetry.get("windows_hashed")
    required = {
        "buffers_submitted",
        "samples_submitted",
        "windows_hashed",
        "fifo_samples_dropped",
        "fifo_windows_dropped",
        "midi_events_dropped",
        "bypassed_buffers",
        "bypassed_samples",
        "events_prepared",
        "udp_sends_attempted",
        "udp_sends_failed",
    }
    if chain_length == 0 or not required.issubset(telemetry):
        return {
            "status": "unknown_coverage",
            "basis": (
                "No routed hash windows were received."
                if chain_length == 0
                else "The plug-in stream did not include every required cumulative counter."
            ),
            "counters": counters,
            "apw:proof_level": "unknown_unobserved",
        }

    loss_count = sum((
        telemetry.get("fifo_samples_dropped", 0),
        telemetry.get("fifo_windows_dropped", 0),
        telemetry.get("midi_events_dropped", 0),
        telemetry.get("bypassed_buffers", 0),
        # Optional (older plugin builds omit it), so not in `required`.
        telemetry.get("midi_unsupported_dropped", 0),
        telemetry.get("udp_sends_failed", 0),
        # A cumulative counter that went backwards means either a spoofed
        # datagram or a restarted plug-in instance; neither supports a claim of
        # complete coverage over this session.
        telemetry_regressions,
        receiver["sequence_gaps"],
        receiver["sequence_out_of_order"],
        receiver["hash_chain_breaks"],
        receiver["stream_evictions"],
    ))
    complete = (
        windows_hashed == chain_length
        and telemetry.get("events_prepared") == receiver["events_received"]
        and loss_count == 0
        and receiver["events_missing_sequence"] == 0
        and receiver["daemon_acknowledgements_sent"] == receiver["packets_received"]
        and receiver["daemon_acknowledgements_failed"] == 0
        and plugin_instance_count == 1
    )
    status = "complete_observed_path" if complete else "partial_observed_path"
    return {
        "status": status,
        "basis": (
            "All submitted routed windows represented by the plug-in counters were received, "
            "and the prepared/received event prefix agrees with no reported FIFO loss, UDP send "
            "failure, sequence gap, chain break, or daemon acknowledgement dispatch failure. "
            "Scope ends at the last received telemetry event; plug-in processing of each ACK is not "
            "observable from the daemon."
            if complete
            else "Routed audio was observed, but one or more counters show or cannot exclude loss."
        ),
        "counters": counters,
        "apw:proof_level": "inferred",
    }


def derive_forgery_analysis(events_snapshot: list[dict[str, object]]) -> dict[str, object]:
    audio = AudioStreamAnalyzer()
    chain = HashChainAnalyzer()
    input_behavior = InputBehaviorAnalyzer()
    for event in events_snapshot:
        event_type = event.get("event_type")
        if event_type == "buffer_hash":
            audio.ingest_buffer_hash(event)
            chain.ingest_buffer_hash(event)
        elif event_type == "audio_transition":
            audio.ingest_transition(event)
        elif event_type == "input_keystroke_stats":
            input_behavior.ingest_keystroke_batch(event)

    def render(report: ForgeryReport) -> dict[str, object]:
        return {
            "suspicion_score": round(report.suspicion_score, 3),
            "sample_count": report.sample_count,
            "flags": [
                {
                    "name": flag.name,
                    "description": flag.description,
                    "severity": flag.severity,
                    "evidence": flag.evidence,
                }
                for flag in report.flags
            ],
        }

    audio_report = audio.analyze()
    chain_report = chain.analyze()
    input_report = input_behavior.analyze()
    input_rendered = render(input_report)
    if input_report.sample_count == 0:
        input_rendered["note"] = (
            "The input_capture layer is not active in this session; "
            "no keystroke statistics were available to analyze."
        )
    return {
        "apw:proof_level": "inferred",
        "method": "statistical_screen_v1",
        "scope": (
            "Statistical screening of the routed-audio stream and hash chain for "
            "synthetic or scripted patterns. An empty flag list is not proof of "
            "authenticity; a flag is a lead, not a verdict."
        ),
        "suspicion_score": round(
            max(
                audio_report.suspicion_score,
                chain_report.suspicion_score,
                input_report.suspicion_score,
            ),
            3,
        ),
        "analyzers": {
            "audio_stream": render(audio_report),
            "hash_chain": render(chain_report),
            "input_behavior": input_rendered,
        },
    }


def _c2pa_ingredient_id(title: str, digest: str) -> str:
    """Stable, collision-resistant id for an ingredient node.

    IMPORTANT: titles come from wire and project data. build_manifest raises on a
    duplicate id and on a title with no usable characters, and a raised
    ManifestError inside the export watcher would cost the whole manifest.
    """
    slug = "".join(ch if ch.isalnum() or ch in "-_." else "-" for ch in title).strip("-.")
    return f"{(slug or 'ingredient')[:48]}-{digest[:8]}"


def _project_sample_ingredients(
    daemon: "Daemon", known_digests: set[str]
) -> tuple[list[tuple[str, str]], list[dict[str, object]]]:
    """Samples the saved project references that the daemon never observed."""
    snapshot = daemon._latest_project_snapshot
    if snapshot is None:
        return [], []
    nodes: list[tuple[str, str]] = []
    unresolved: list[dict[str, object]] = []

    def unresolvable(reference: str, reason: str) -> None:
        unresolved.append({
            "reference": reference,
            "reason": reason,
            "apw:proof_level": "unknown_unobserved",
        })

    for reference in sorted(snapshot.sample_refs):
        if len(nodes) >= _MAX_PROJECT_INGREDIENTS:
            unresolvable(
                reference,
                f"more than {_MAX_PROJECT_INGREDIENTS} project sample references; "
                "the remainder were not hashed",
            )
            break
        path = Path(reference)
        if not path.is_absolute():
            unresolvable(reference, "project-relative reference was not resolved to a file")
            continue
        try:
            if not path.is_file():
                unresolvable(reference, "referenced file is not present on this machine")
                continue
            if path.stat().st_size > _MAX_INGREDIENT_HASH_BYTES:
                unresolvable(reference, "referenced file exceeds the ingredient hashing limit")
                continue
            digest = sha256_file(path)
        except OSError as exc:
            unresolvable(reference, f"referenced file could not be read: {exc}")
            continue
        if digest in known_digests:
            continue
        known_digests.add(digest)
        nodes.append((path.name, digest))
    return nodes, unresolved


def _build_c2pa_claim(
    daemon: "Daemon",
    *,
    export_path: Path,
    export_hash: str,
    builder: ManifestBuilder,
    signed_asset_path: Path,
    sidecar_path: Path,
    association: dict[str, object],
) -> dict[str, object]:
    """Sign the detected export into a real C2PA claim and read it back.

    The export file itself is never rewritten: export.sha256 is committed before
    this runs, and an in-place rewrite would make the manifest describe a file
    that no longer exists.
    """
    provider = getattr(daemon, "_provenance_provider", None)
    if provider is None:
        # IMPORTANT: log, do not merely record. Without this the daemon log of a
        # run with the headline feature entirely off is indistinguishable from a
        # healthy one, and the presenter watches a normal-looking terminal.
        log.error(
            "No provenance provider is available; %s will carry no signed C2PA claim",
            export_path.name,
        )
        return unavailable_c2pa_claim(
            "No provenance provider is available to issue signing material."
        )
    try:
        from daemon.c2pa_engine.manifest import Ingredient, ManifestSpec, build_manifest
        from daemon.c2pa_engine.signer import build_signer, detect_format, sign_asset
        from daemon.c2pa_engine.verifier import verify_asset
    except ImportError as exc:
        log.error(
            "The C2PA engine is unavailable in this runtime (%s); %s will carry no signed "
            "claim and no signed asset will be written",
            exc, export_path.name,
        )
        return unavailable_c2pa_claim(f"The C2PA engine is unavailable in this runtime: {exc}")

    known_digests = {i.sha256 for i in builder.ingredients if i.sha256}
    project_nodes, unresolved = _project_sample_ingredients(daemon, set(known_digests))

    ingredients: list[object] = []
    nodes: list[dict[str, object]] = []
    seen_ids: set[str] = set()

    def add_node(
        title: str,
        digest: str,
        relationship: str,
        proof_level: str,
        meaning: str,
        digest_kind: str = "file_sha256",
    ) -> None:
        if not digest:
            unresolved.append({
                "reference": title,
                "reason": "no digest was available, so the node cannot be recorded as an ingredient",
                "apw:proof_level": "unknown_unobserved",
            })
            return
        ingredient_id = _c2pa_ingredient_id(title, digest)
        if ingredient_id in seen_ids:
            return
        seen_ids.add(ingredient_id)
        ingredients.append(Ingredient(
            title=title,
            relationship=relationship,
            proof_level=proof_level,
            sha256=digest,
            ingredient_id=ingredient_id,
            digest_kind=digest_kind,
        ))
        nodes.append({
            "title": title,
            "ingredient_id": ingredient_id,
            "relationship": relationship,
            "sha256": digest,
            "digest_meaning": meaning,
            "apw:proof_level": proof_level,
        })

    # IMPORTANT: a componentOf ingredient plus its synthesized c2pa.placed action
    # states as signed fact that the routed stem is part of this export. When the
    # feature comparison did not establish that link, the travelling artifact must
    # not assert it; the stem is disclosed as an unresolved reference instead.
    association_status = str(association.get("status", "not_established"))
    association_established = association_status == _ASSOCIATION_ESTABLISHED
    association_note: str | None = None
    if not association_established and builder.stems:
        association_note = (
            f"Routed audio was observed, but the stem-to-export comparison is "
            f"{association_status}"
            + (f" ({association.get('reason')})" if association.get("reason") else "")
            + ". The observed stem is therefore not claimed as an ingredient of this export."
        )
    for stem in builder.stems:
        if not association_established:
            unresolved.append({
                "reference": stem.stem_id,
                "reason": association_note,
                "apw:proof_level": "unknown_unobserved",
            })
            continue
        add_node(
            stem.stem_id, stem.hash_chain_root, "componentOf",
            stem.proof_level, _STEM_DIGEST_MEANING, "hash_chain_root",
        )
    for ingredient in builder.ingredients:
        add_node(
            ingredient.file_name, ingredient.sha256, "inputTo",
            ingredient.proof_level, _OBSERVED_SAMPLE_DIGEST_MEANING,
        )
    for file_name, digest in project_nodes:
        add_node(
            file_name, digest, "inputTo",
            "unknown_unobserved", _UNOBSERVED_SAMPLE_DIGEST_MEANING,
        )

    try:
        asset_format = detect_format(export_path)
        material = provider.issue_signing_material()
        signer = build_signer(
            material.certificate_chain_pem, material.private_key_handle, material.algorithm
        )
        c2pa_manifest = build_manifest(ManifestSpec(
            title=export_path.name,
            ingredients=ingredients,
            # The travelling artifact must disclose at least what the local
            # manifest does; this defaulted to a single hardcoded item.
            unobserved=_unobserved_claims(list(builder.unobserved), association_note),
            mime=asset_format.mime,
        ))
        signing = sign_asset(
            export_path, signed_asset_path, c2pa_manifest, signer, sidecar_path=sidecar_path
        )
        verification = verify_asset(
            signing.asset_path,
            signing.mime,
            trust_anchors_pem=material.trust_anchor_pem,
            sidecar_manifest=signing.manifest_path if signing.mode == "sidecar" else None,
        )
        identity = dict(provider.identity())
    except Exception as exc:
        log.error(
            "Could not produce a C2PA claim for %s; the export will carry no signed claim",
            export_path.name, exc_info=True,
        )
        detail = " ".join(str(exc).split())[:400]
        return unavailable_c2pa_claim(f"{type(exc).__name__}: {detail}")

    signed_hash = sha256_file(signing.asset_path)
    manifest_dir = daemon.manifest_dir
    return {
        "status": signing.mode,
        "mime": signing.mime,
        "claim_generator": c2pa_manifest["claim_generator_info"],
        "signed_asset": {
            "file_name": signing.asset_path.name,
            "file_path": str(signing.asset_path.resolve()),
            "relative_path": _relative_artifact(signing.asset_path, manifest_dir),
            "sha256": signed_hash,
            "apw:proof_level": "directly_observed",
        },
        "sidecar_manifest": (
            _relative_artifact(signing.manifest_path, manifest_dir)
            if signing.mode == "sidecar" and signing.manifest_path is not None
            else None
        ),
        "source_export_sha256": export_hash,
        "source_sha256_matches_export": signing.binding.get("source_sha256") == export_hash,
        "hard_binding": dict(signing.binding),
        "validation": {
            "state": verification.state,
            "library_validation_state": verification.validation_state,
            "failure_codes": list(verification.failure_codes),
            "assertion_labels": list(verification.assertion_labels),
            "trust_evaluated": verification.trust_evaluated,
            "trust_anchor_scope": "self_issued_local_root_only",
            "detail": verification.detail,
        },
        "signer": {
            **identity,
            "signer_identity": "not_established",
            "trust_anchor_pem": material.trust_anchor_pem.decode("ascii"),
        },
        "ingredients": nodes,
        "unresolved_ingredient_references": unresolved,
        "scope": C2PA_CLAIM_SCOPE,
        "apw:proof_level": "directly_observed",
    }


def _relative_artifact(path: Path, manifest_dir: Path) -> str:
    """Manifest-relative artifact reference.

    IMPORTANT: relative_to() raises for a sidecar, whose asset lives under
    exports/ rather than the manifest directory, and the old fallback wrote the
    presenter's absolute local path (username included) into the signed handoff.
    """
    return Path(os.path.relpath(path, start=manifest_dir)).as_posix()


def _as_int(value: object) -> int:
    """Wire values reach the manifest as-is; a bad one must not abort the export."""
    try:
        return int(value or 0)
    except (TypeError, ValueError):
        return 0


def _unobserved_claims(names: list[str], association_note: str | None):
    from daemon.c2pa_engine.manifest import UnobservedClaim

    claims = []
    for name in names:
        if name.startswith("layer_") and name.endswith("_not_active"):
            layer = name[len("layer_"):-len("_not_active")]
            evidence = f"The {layer} observation layer was not active in this capture session."
        else:
            evidence = _UNOBSERVED_EVIDENCE.get(
                name, "This aspect was outside the capture path and was never observed."
            )
        claims.append(UnobservedClaim(claim=name, value=False, evidence=evidence))
    if association_note is not None:
        claims.append(UnobservedClaim(
            claim="observed_stem_linked_to_export",
            value=False,
            evidence=association_note,
        ))
    return claims


def generate_manifest(daemon: "Daemon", export_path: Path, export_version: int = 1) -> Path:
    daemon._last_export_path = export_path
    export_hash = sha256_file(export_path)
    stat = export_path.stat()
    export_metadata = extract_audio_metadata(export_path)
    builder = ManifestBuilder(
        session_id=daemon.session_id,
    )
    builder.set_export(ExportEvidence(
        file_path=str(export_path),
        file_name=export_path.name,
        sha256=export_hash,
        format=export_path.suffix.lower().lstrip("."),
        file_size_bytes=stat.st_size,
        duration_seconds=export_metadata.get("duration_seconds"),
        exported_at=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        sample_rate_hz=export_metadata.get("sample_rate"),
        channel_count=export_metadata.get("channels"),
        export_version=export_version,
    ))

    with daemon._session_lock:
        events_snapshot = list(daemon._session_events)
        feature_snapshot = list(daemon._feature_events)
        first_hash_event = dict(daemon._first_hash_event or {})
        last_hash_event = dict(daemon._last_hash_event or {})
        chain_length = daemon._buffer_hash_count
        plugin_instance_ids = tuple(sorted(daemon._plugin_instance_ids))

    first_hash_ms = _as_int(first_hash_event.get("source_timestamp_ms"))
    last_hash_ms = _as_int(last_hash_event.get("source_timestamp_ms"))
    first_received_at = str(first_hash_event.get("received_at") or "") or None
    last_received_at = str(last_hash_event.get("received_at") or "") or None
    last_window_hash = str(last_hash_event.get("window_hash") or "")
    chain_genesis = str(first_hash_event.get("prev_hash") or "genesis")
    stem_sr = _as_int(last_hash_event.get("sample_rate_hz"))
    stem_ch = _as_int(last_hash_event.get("channel_count"))

    for event in events_snapshot:
        et = event.get("event_type")
        if et == "sample_file_observed":
            builder.add_ingredient(IngredientEvidence(
                file_name=str(event.get("file_name", "")),
                sha256=str(event.get("sha256", "")),
                proof_level=str(event.get("proof_level", "unknown_unobserved")),
                correlation_confidence=None,
                audio_fingerprint=event.get("audio_fingerprint"),
            ))
        elif et == "composite_edit":
            builder.add_composite_edit(event)

    if chain_length > 0:
        builder.add_stem(StemEvidence(
            stem_id=daemon.stem_id,
            hash_chain_root=last_window_hash,
            hash_chain_length=chain_length,
            first_observed_ms=first_hash_ms,
            last_observed_ms=last_hash_ms,
            sample_rate_hz=stem_sr,
            channel_count=stem_ch,
            source_category=daemon.source_category,
            proof_level="directly_observed",
            source_category_proof_level=daemon.source_category_proof_level,
            hash_chain_genesis=chain_genesis,
            first_received_at=first_received_at,
            last_received_at=last_received_at,
            plugin_instance_ids=plugin_instance_ids,
        ))

    builder.coverage = daemon._derive_coverage(chain_length)
    association = associate_export(export_path, feature_snapshot)
    # IMPORTANT: past tense only when the comparison actually ran. The fight card
    # renders basis and never reason, so the static text left an unavailable
    # association claiming on screen that features had been compared.
    if association.get("status") == "unavailable":
        basis = (
            "No routed/export feature comparison was performed: "
            f"{association.get('reason', 'the comparison was unavailable')}. "
            "This is not evidence that routed audio is absent from the export."
        )
    else:
        basis = (
            "A bounded sequence of relative RMS, zero-crossing, crest-factor, and coarse energy-envelope "
            "features emitted from accepted routed plug-in windows was compared with equivalent streaming-"
            "extracted export features using time-offset search. The relationship remains inferred and does "
            "not establish complete routing."
        )
    association.update({
        "capture_session_id": daemon.session_id,
        "stem_ids": [daemon.stem_id] if chain_length else [],
        "export_file_name": export_path.name,
        "basis": basis,
    })
    builder.audio_association = association
    daemon._last_association_status = str(association.get("status", "not_established"))
    builder.session_diagnostics = daemon._session_diagnostics()

    all_layers = {"audio_buffer", "transport", "midi", "session",
                  "sample_watcher", "project_differ", "input_capture",
                  "screen_observer"}
    with daemon._session_lock:
        active_layers = set(daemon._active_layers)
    missing_layers = sorted(all_layers - active_layers)
    builder.unobserved = list(builder.unobserved)
    for layer in missing_layers:
        builder.unobserved.append(f"layer_{layer}_not_active")

    builder.set_forgery_report(daemon._derive_forgery_analysis(events_snapshot))

    suffix = daemon.manifest_suffix(export_version)
    manifest_path = daemon.manifest_dir / f"{export_path.stem}{suffix}_manifest.json"
    report_path = daemon.manifest_dir / f"{export_path.stem}{suffix}_provenance.html"
    artifact_dir = daemon.manifest_dir / "artifacts"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    verification_path = artifact_dir / f"{export_path.stem}{suffix}_verification.json"
    handoff_path = artifact_dir / f"{export_path.stem}{suffix}_handoff.json"
    bundle_index_path = artifact_dir / f"{export_path.stem}{suffix}_bundle_index.json"
    bundle_path = artifact_dir / f"{export_path.stem}{suffix}_evidence_bundle.zip"
    # IMPORTANT: the signed copy goes under manifest_dir/artifacts, never over the
    # export. export.sha256 is already committed, and the export watcher scans
    # export_dir non-recursively, so a signed sibling here is not re-detected.
    signed_asset_path = artifact_dir / f"{export_path.stem}{suffix}_c2pa{export_path.suffix}"
    sidecar_path = artifact_dir / f"{export_path.stem}{suffix}{export_path.suffix}.c2pa"
    builder.set_c2pa_claim(_build_c2pa_claim(
        daemon,
        export_path=export_path,
        export_hash=export_hash,
        builder=builder,
        signed_asset_path=signed_asset_path,
        sidecar_path=sidecar_path,
        association=association,
    ))

    evidence_hashes: dict[str, str] = {}
    evidence_files: dict[str, dict[str, object]] = {}
    for evidence_file in sorted(daemon.evidence_dir.glob("*.jsonl")):
        byte_length = evidence_file.stat().st_size
        digest = sha256_prefix(evidence_file, byte_length)
        evidence_hashes[evidence_file.name] = digest
        evidence_files[evidence_file.name] = {
            "sha256": digest,
            "byte_length": byte_length,
            "binding_scope": "file_prefix_at_manifest_creation",
        }

    manifest = builder.build()

    snap = daemon._latest_project_snapshot
    if snap is not None:
        manifest["session_facts"] = {
            "apw:proof_level": "inferred",
            "observation_basis": (
                "Structural facts inferred by parsing the saved Ableton .als file; "
                "Ableton does not provide this project with a supported semantic API."
            ),
            "bpm": snap.transport_bpm,
            "time_signature": f"{snap.transport_time_signature[0]}/{snap.transport_time_signature[1]}",
            "loop_on": snap.transport_loop_on,
            "track_count": snap.track_count,
            "clip_count": snap.clip_count,
            "sample_refs": sorted(snap.sample_refs),
            "tracks": [
                {
                    "name": t.name,
                    "type": t.track_type,
                    "devices": list(t.devices),
                    "device_presets": list(t.device_presets),
                    "sample_paths": list(t.sample_paths),
                    "clips": [
                        {
                            "name": c.name,
                            "position_beats": c.position_beats,
                            "length_beats": c.length_beats,
                            "sample_ref": c.sample_ref,
                            "warp_on": c.warp_on,
                            "is_midi": c.is_midi,
                        }
                        for c in t.clips
                    ],
                    "clip_count": t.clip_count,
                    "midi_note_count": t.midi_note_count,
                    "automation_point_count": t.automation_point_count,
                    "routing_input": t.routing_input,
                    "routing_output": t.routing_output,
                    "sends": [{"target": s.target, "level": s.level} for s in t.sends],
                    "group_id": t.group_id,
                    "is_frozen": t.is_frozen,
                    "color_index": t.color_index,
                }
                for t in snap.tracks
            ],
        }
    manifest["evidence_binding"] = {
        "evidence_directory": str(daemon.evidence_dir.resolve()),
        "evidence_file_hashes": evidence_hashes,
        "evidence_files": evidence_files,
        "last_window_hash": last_window_hash,
        "chain_length": chain_length,
        "apw:proof_level": "directly_observed",
    }
    manifest["capture_session"]["started_at"] = daemon._session_started_at
    manifest["capture_session"]["state_at_manifest"] = "active"
    receipt_summary = daemon.receiver.receipt_summary()
    manifest["daemon_receipt_acknowledgement"] = receipt_summary
    manifest["claim_summary"].insert(2, {
        "claim": "daemon_receipt_acknowledgement",
        "value": receipt_summary["status"],
        "evidence": (
            f"Daemon dispatched {receipt_summary['counters']['sent']} local ACK packets; "
            "plug-in processing of each packet is outside daemon observability."
        ),
        "apw:proof_level": receipt_summary["apw:proof_level"],
    })

    manifest["presentation"] = {
        "html_report": report_path.name if daemon.generate_html_report else None,
        "derived_from": manifest_path.name,
        "verifier_result": str(verification_path.relative_to(daemon.manifest_dir)),
        "downstream_handoff": str(handoff_path.relative_to(daemon.manifest_dir)),
        "bundle_index": (
            str(bundle_index_path.relative_to(daemon.manifest_dir))
            if daemon.generate_html_report else None
        ),
        "evidence_bundle": (
            str(bundle_path.relative_to(daemon.manifest_dir))
            if daemon.generate_html_report else None
        ),
        "c2pa_signed_asset": (manifest["c2pa_claim"].get("signed_asset") or {}).get("relative_path"),
        "c2pa_sidecar_manifest": manifest["c2pa_claim"].get("sidecar_manifest"),
        "apw:proof_level": "directly_observed",
    }
    manifest["downstream_registration_handoff"] = daemon._build_handoff(
        export_hash=export_hash,
        association=association,
        coverage=builder.coverage,
        evidence_files=evidence_files,
        manifest_name=manifest_path.name,
        bundle_name=bundle_path.name if daemon.generate_html_report else None,
        bundle_index_name=bundle_index_path.name if daemon.generate_html_report else None,
        c2pa_claim=manifest["c2pa_claim"],
        chain_root=last_window_hash or None,
        chain_length=chain_length,
    )

    if daemon._time_anchor is not None:
        manifest["time_anchor"] = daemon._time_anchor.anchor_record(export_hash)

    if chain_length > 0 and last_window_hash:
        # Added before signing so both manifest signatures cover the binding.
        try:
            binding = daemon._hw_provider.bind_chain_root(last_window_hash)
            manifest["hardware_binding"] = {
                **dataclasses.asdict(binding),
                "hardware_attested": not isinstance(daemon._hw_provider, SoftwareProvider),
                "apw:proof_level": (
                    "unknown_unobserved"
                    if isinstance(daemon._hw_provider, SoftwareProvider)
                    else "directly_observed"
                ),
            }
        except Exception:
            log.warning("Could not bind hash chain root to device", exc_info=True)

    try:
        manifest["portable_signature"] = daemon._portable_signer.sign_manifest(manifest)
    except Exception:
        log.exception("Could not create portable Ed25519 signature")

    try:
        identity = daemon._hw_provider.device_identity()
        manifest_bytes = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
        signature = daemon._hw_provider.sign(manifest_bytes)
        signed_content_hash = hashlib.sha256(manifest_bytes).hexdigest()
        # Entangles the software signature, so it cannot precede signing; it
        # lives inside manifest_signature because the verifier excludes that
        # key when recomputing signed_content_hash (verify.py).
        cosignature = daemon._hw_provider.cosign_checkpoint(
            content_hash=signed_content_hash,
            software_signature=signature.hex(),
            previous_cosignature_hash=daemon._last_cosignature_hash,
        )
        manifest["manifest_signature"] = {
            "algorithm": identity.algorithm,
            "device_id": identity.device_id,
            "public_key_hex": identity.public_key_hex,
            "signature_hex": signature.hex(),
            "signed_content_hash": signed_content_hash,
            "trust_scope": (
                "local_software_integrity"
                if isinstance(daemon._hw_provider, SoftwareProvider)
                else "hardware_provider"
            ),
            "hardware_attested": not isinstance(daemon._hw_provider, SoftwareProvider),
            "apw:proof_level": (
                "unknown_unobserved"
                if isinstance(daemon._hw_provider, SoftwareProvider)
                else "directly_observed"
            ),
            "notes": (
                "The local HMAC seal detects changes when checked with the same secret key; "
                "it is not hardware attestation or third-party identity verification."
                if isinstance(daemon._hw_provider, SoftwareProvider)
                else "Signature produced by the configured hardware provider."
            ),
            "hardware_cosignature": {
                **dataclasses.asdict(cosignature),
                "counter_scope": daemon._hw_provider.counter_scope(),
                "apw:proof_level": (
                    "unknown_unobserved"
                    if isinstance(daemon._hw_provider, SoftwareProvider)
                    else "directly_observed"
                ),
            },
        }
        daemon._last_cosignature_hash = cosignature.entangled_hash
    except Exception:
        log.warning("Could not sign manifest", exc_info=True)

    manifest_path.parent.mkdir(parents=True, exist_ok=True)
    with manifest_path.open("w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2, ensure_ascii=False)
        f.write("\n")
    log.info("Manifest written: %s", manifest_path)
    handoff_path.write_text(
        json.dumps(manifest["downstream_registration_handoff"], indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    from daemon.verify import verify_manifest

    # The verifier checks the fight card link. Materialize the unsigned derived
    # presentation first, then store the verifier result, then render the final
    # presentation with that local result. The signed JSON manifest is unchanged.
    if daemon.generate_html_report:
        write_html_report(dict(manifest), report_path)
    verification = verify_manifest(
        manifest_path,
        signing_key_path=daemon._signing_key_path,
        public_key_path=daemon._portable_signer.public_key_path,
        trust_anchor_path=daemon._provenance_store / "ca" / "root_cert.pem",
    )
    verification_path.write_text(
        json.dumps(verification.to_dict(), indent=2, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    if daemon.generate_html_report:
        report_manifest = dict(manifest)
        report_manifest["local_verification_summary"] = verification.to_dict()
        write_html_report(report_manifest, report_path)
        log.info("Fight-card report written: %s", report_path)
        try:
            create_evidence_bundle(
                manifest_path=manifest_path,
                report_path=report_path,
                verification_path=verification_path,
                handoff_path=handoff_path,
                index_path=bundle_index_path,
                bundle_path=bundle_path,
                signer=daemon._portable_signer,
            )
            log.info("Evidence bundle written: %s", bundle_path)
        except Exception:
            log.exception("Could not create deterministic evidence bundle")
        if daemon.open_artifacts:
            try:
                subprocess.Popen(["open", str(report_path)])
            except OSError:
                log.warning("Could not open fight card automatically", exc_info=True)
    daemon._last_verifier_outcome = verification.outcome
    daemon._last_manifest_path = manifest_path
    daemon._last_report_path = report_path if daemon.generate_html_report else None
    daemon._last_verification_path = verification_path
    daemon._last_handoff_path = handoff_path
    daemon._last_bundle_index_path = bundle_index_path if bundle_index_path.is_file() else None
    daemon._last_bundle_path = bundle_path if bundle_path.is_file() else None
    daemon._last_export_path = export_path
    daemon._last_sdk_receipt_path = None
    daemon._last_sdk_record_id = None
    daemon._last_sdk_verification_status = None
    daemon._last_sdk_error = None
    if daemon.sdk_adapter_enabled:
        try:
            from daemon.sdk_adapter import (
                CaptureAdapterInvocation,
                ensure_development_key,
                run_capture_adapter,
            )

            if not bundle_path.is_file():
                raise RuntimeError(
                    "the SDK adapter requires the completed evidence bundle; enable the HTML/bundle output"
                )
            sdk_key = ensure_development_key(daemon.sdk_development_key)
            sdk_receipt = artifact_dir / f"{export_path.stem}{suffix}_sdk_receipt.json"
            sdk_sidecar = artifact_dir / f"{export_path.stem}{suffix}_sdk_record.json"
            bundle_digest = sha256_file(bundle_path)
            sdk_result = run_capture_adapter(
                CaptureAdapterInvocation(
                    export=export_path,
                    capture_manifest=manifest_path,
                    handoff=handoff_path,
                    evidence_bundle=bundle_path,
                    key=sdk_key,
                    receipt=sdk_receipt,
                    sidecar=sdk_sidecar,
                    evidence_bundle_sha256=bundle_digest,
                    registry=daemon.sdk_registry,
                ),
                cli=daemon.sdk_cli,
            )
            sign_result = sdk_result.get("sign")
            verification_result = sdk_result.get("verification")
            daemon._last_sdk_receipt_path = sdk_receipt
            daemon._last_sdk_record_id = (
                str(sign_result.get("record_id")) if isinstance(sign_result, dict) else None
            )
            daemon._last_sdk_verification_status = (
                str(verification_result.get("status"))
                if isinstance(verification_result, dict)
                else None
            )
            log.info("Development SDK adapter receipt written: %s", sdk_receipt)
        except Exception as exc:
            daemon._last_sdk_error = str(exc)
            log.exception("Development SDK adapter failed explicitly")
    daemon._write_status(
        "active"
        if daemon._plugin_seen and time.monotonic() - daemon._last_plugin_event_monotonic < 3.0
        else "idle"
    )
    if chain_length == 0:
        log.warning("Export was hashed, but no routed-audio hash events were received")
    return manifest_path
