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
)
from daemon.report import write_html_report
from daemon.sample_watcher.watcher import extract_audio_metadata

if TYPE_CHECKING:
    from daemon.__main__ import Daemon

log = logging.getLogger(__name__)


def derive_coverage(daemon: "Daemon", chain_length: int) -> dict[str, object]:
    receiver = daemon.receiver.diagnostics()
    with daemon._session_lock:
        telemetry = dict(daemon._latest_plugin_telemetry)
        plugin_instance_count = len(daemon._plugin_instance_ids)
    counters: dict[str, int] = {
        **telemetry,
        **receiver,
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
        telemetry.get("udp_sends_failed", 0),
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

    first_hash_ms = int(first_hash_event.get("source_timestamp_ms") or 0)
    last_hash_ms = int(last_hash_event.get("source_timestamp_ms") or 0)
    first_received_at = str(first_hash_event.get("received_at") or "") or None
    last_received_at = str(last_hash_event.get("received_at") or "") or None
    last_window_hash = str(last_hash_event.get("window_hash") or "")
    chain_genesis = str(first_hash_event.get("prev_hash") or "genesis")
    stem_sr = int(last_hash_event.get("sample_rate_hz") or 0)
    stem_ch = int(last_hash_event.get("channel_count") or 0)

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
    association.update({
        "capture_session_id": daemon.session_id,
        "stem_ids": [daemon.stem_id] if chain_length else [],
        "export_file_name": export_path.name,
        "basis": (
            "A bounded sequence of relative RMS, zero-crossing, crest-factor, and coarse energy-envelope "
            "features emitted from accepted routed plug-in windows was compared with equivalent streaming-"
            "extracted export features using time-offset search. The relationship remains inferred and does "
            "not establish complete routing."
        ),
    })
    builder.audio_association = association
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

    suffix = "" if export_version == 1 else f"_v{export_version:03d}"
    manifest_path = daemon.manifest_dir / f"{export_path.stem}{suffix}_manifest.json"
    report_path = daemon.manifest_dir / f"{export_path.stem}{suffix}_provenance.html"
    artifact_dir = daemon.manifest_dir / "artifacts"
    artifact_dir.mkdir(parents=True, exist_ok=True)
    verification_path = artifact_dir / f"{export_path.stem}{suffix}_verification.json"
    handoff_path = artifact_dir / f"{export_path.stem}{suffix}_handoff.json"
    bundle_index_path = artifact_dir / f"{export_path.stem}{suffix}_bundle_index.json"
    bundle_path = artifact_dir / f"{export_path.stem}{suffix}_evidence_bundle.zip"
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
            "hardware_cosignature": dataclasses.asdict(cosignature),
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
    verification = verify_manifest(manifest_path)
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
    daemon._write_status(
        "active"
        if daemon._plugin_seen and time.monotonic() - daemon._last_plugin_event_monotonic < 3.0
        else "idle"
    )
    if chain_length == 0:
        log.warning("Export was hashed, but no routed-audio hash events were received")
    return manifest_path
