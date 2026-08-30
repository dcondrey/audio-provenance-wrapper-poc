"""Status-file rendering, readiness guidance, session diagnostics, and the
downstream registration handoff record.

Extracted from ``daemon.Daemon._derive_readiness`` / ``_session_diagnostics``
/ ``_build_handoff`` / ``_status_link`` / ``_write_status``. Most functions
take the ``Daemon`` instance because the source methods read a broad slice
of daemon session state; ``status_link`` is the one genuinely pure piece and
is extracted as such.
"""

from __future__ import annotations

import json
import os
import threading
from pathlib import Path
from typing import TYPE_CHECKING

from daemon.audio_association import MIN_ROUTED_COVERAGE
from daemon.common import utc_timestamp
from daemon.dashboard import write_dashboard

if TYPE_CHECKING:
    from daemon.__main__ import Daemon

def derive_readiness(daemon: "Daemon") -> dict[str, object]:
    """Operator guidance from the same counters that grade coverage.

    Encodes the live-pass timing rules so the dashboard flags them in
    realtime instead of leaving them as runbook folklore.
    """
    with daemon._session_lock:
        telemetry = dict(daemon._latest_plugin_telemetry)
        chain_length = daemon._buffer_hash_count
        last_hash = dict(daemon._last_hash_event) if daemon._last_hash_event else {}
    alerts: list[str] = []
    windows_hashed = telemetry.get("windows_hashed")
    # In-flight UDP lets the plug-in counter lead the received count by a
    # little; a stale instance leads by its entire pre-session life.
    if windows_hashed is not None and windows_hashed > chain_length + 16:
        alerts.append(
            f"Plug-in instance predates this daemon session (plug-in hashed "
            f"{windows_hashed} windows, daemon received {chain_length}). "
            "Delete and re-add the device; coverage will otherwise grade partial."
        )
    if telemetry.get("fifo_samples_dropped", 0) or telemetry.get("fifo_windows_dropped", 0):
        alerts.append(
            "Plug-in audio FIFO overflowed (offline render outruns the realtime "
            "hasher). Deactivate the device before File → Export and start a "
            "fresh take; coverage will otherwise grade partial."
        )
    if telemetry.get("midi_events_dropped", 0):
        alerts.append(
            "Plug-in MIDI FIFO overflowed; MIDI evidence was lost and coverage "
            "will grade partial."
        )
    if telemetry.get("midi_unsupported_dropped", 0):
        alerts.append(
            "Unsupported MIDI message types (pitch bend, aftertouch, sysex) were "
            "not captured; MIDI evidence is incomplete and coverage will grade partial."
        )
    window_size = int(last_hash.get("window_size_samples") or 0)
    sample_rate = int(last_hash.get("sample_rate_hz") or 0)
    routed_seconds = (
        chain_length * window_size / sample_rate if window_size and sample_rate else 0.0
    )
    if routed_seconds:
        min_export_seconds = max(1, int(routed_seconds * MIN_ROUTED_COVERAGE + 0.999))
        export_guidance = (
            f"Render at least {min_export_seconds}s (routed audio observed so far: "
            f"{int(routed_seconds)}s). A shorter export dilutes the association "
            f"below the {int(MIN_ROUTED_COVERAGE * 100)}% overlap floor."
        )
    else:
        min_export_seconds = None
        export_guidance = "Waiting for routed audio before export guidance is available."
    return {
        "ready_to_export": bool(chain_length) and not alerts,
        "alerts": alerts,
        "routed_seconds": round(routed_seconds, 1),
        "min_export_seconds": min_export_seconds,
        "export_guidance": export_guidance,
    }


def session_diagnostics(daemon: "Daemon") -> dict[str, object]:
    return {
        "receiver": daemon.receiver.diagnostics(),
        "correlation": {
            "buffer_events": daemon.correlation.buffer_size,
            "buffer_max_events": daemon.correlation.max_buffer_events,
            "capacity_drops": daemon.correlation.capacity_drops,
            "duplicate_matches_suppressed": daemon.correlation.duplicate_suppressions,
            "composite_events_emitted": daemon.correlation.emitted_count,
            "clock": "daemon_monotonic_ms",
        },
        "session_memory": {
            "events_retained": len(daemon._session_events),
            "max_events": daemon._max_session_events,
            "events_dropped_from_memory_only": daemon._session_event_drops,
        },
        "daemon_receipt_acknowledgement": daemon.receiver.receipt_summary(),
        "apw:proof_level": "directly_observed",
    }


def build_handoff(
    daemon: "Daemon",
    *,
    export_hash: str,
    association: dict[str, object],
    coverage: dict[str, object] | None,
    evidence_files: dict[str, dict[str, object]],
    manifest_name: str,
    bundle_name: str | None,
    bundle_index_name: str | None,
) -> dict[str, object]:
    return {
        "record_type": "downstream_provenance_registration_handoff",
        "status": "candidate_input_not_submitted",
        "capture_session_id": daemon.session_id,
        "export_hard_hash": {
            "algorithm": "sha256",
            "value": export_hash,
            "apw:proof_level": "directly_observed",
        },
        "routed_observation_commitment": {
            "hash_chain_root": str((daemon._last_hash_event or {}).get("window_hash", "")) or None,
            "hash_chain_length": daemon._buffer_hash_count,
            "apw:proof_level": (
                "directly_observed" if daemon._buffer_hash_count else "unknown_unobserved"
            ),
        },
        "coverage": coverage,
        "audio_association": association,
        "creator_declarations": [{
            "name": "source_category",
            "value": daemon.source_category,
            "apw:proof_level": daemon.source_category_proof_level,
        }],
        "signing_key": {
            "algorithm": "Ed25519",
            "public_key_hex": daemon._portable_signer.public_key_hex(),
            "public_key_file": str(daemon._portable_signer.public_key_path),
            "trust_scope": "self_generated_demo_key_integrity",
            "signer_identity": "not_established",
            "apw:proof_level": "unknown_unobserved",
        },
        "evidence_bundle": {
            "manifest": str((daemon.manifest_dir / manifest_name).resolve()),
            "export": str(daemon._last_export_path.resolve()) if daemon._last_export_path else None,
            "evidence_directory": str(daemon.evidence_dir.resolve()),
            "files": evidence_files,
            "downloadable_archive": f"artifacts/{bundle_name}" if bundle_name else None,
            "signed_bundle_index": f"artifacts/{bundle_index_name}" if bundle_index_name else None,
            "index_scope": "all archive payload entries; the signed index is not self-hashed",
            "apw:proof_level": "directly_observed",
        },
        "tentative_c2pa_assertion_mapping": {
            "status": "mapping_only_not_a_production_c2pa_claim",
            "assertions": ["c2pa.hash.data", "c2pa.ingredient", "c2pa.actions", "apw.unobserved"],
            "apw:proof_level": "inferred",
        },
        "missing_downstream_requirements": [
            "verified creator or institution identity",
            "author-controlled credential and key policy",
            "production certificate chain",
            "audio-native soft binding or watermark",
            "resilient recovery from the audio",
            "registry publication",
            "production C2PA claim generation and conformance validation",
            "consent and rights verification",
        ],
        "boundary": (
            "This neutral handoff is not a provider-specific API payload and claims no compatibility "
            "with proprietary watermark, recovery, identity, signing, or registry technology."
        ),
    }


def status_link(path: Path | None, status_dir: Path) -> str | None:
    """Dashboard-relative link; relative_to raises when --manifest-dir and the
    status dir do not share a root, which killed the export-watcher thread."""
    if path is None:
        return None
    return os.path.relpath(path, start=status_dir)


def write_status(daemon: "Daemon", state: str) -> None:
    if state != "stopped" and (
        daemon.receiver.rejected_count > 0 or daemon.receiver.hash_chain_break_count > 0
    ):
        state = "error"
    with daemon._session_lock:
        plugin_instance_ids = sorted(daemon._plugin_instance_ids)
        plugin_telemetry = dict(daemon._latest_plugin_telemetry)
    data = {
        "product": "Routed Audio Evidence Adapter",
        "state": state,
        "updated_at": utc_timestamp(),
        "session_id": daemon.session_id,
        "stem_id": daemon.stem_id,
        "plugin_instance_ids": plugin_instance_ids,
        "trust_boundary": (
            "Only routed plug-in audio and local filesystem exports are observed. "
            "Identity, authorship, rights, consent, bypassed paths, and downstream registration remain unestablished. "
            "Verified, changed, untrusted, and not_found are local POC integrity outcomes, not registry outcomes."
        ),
        "coverage": daemon._derive_coverage(daemon._buffer_hash_count),
        "readiness": daemon._derive_readiness(),
        "counts": {
            **plugin_telemetry,
            **daemon.receiver.diagnostics(),
            "udp_sends_locally_emitted": max(
                0,
                plugin_telemetry.get("udp_sends_attempted", 0)
                - plugin_telemetry.get("udp_sends_failed", 0),
            ),
            "buffer_hash_events_received": daemon._buffer_hash_count,
        },
        "pipeline": {
            "plugin_observed": "complete" if daemon._buffer_hash_count else "waiting",
            "plugin_emitted": (
                "degraded"
                if plugin_telemetry.get("udp_sends_failed", 0)
                else "complete"
                if plugin_telemetry.get("udp_sends_attempted", 0)
                else "waiting"
            ),
            "daemon_received": "complete" if daemon.receiver.event_count else "waiting",
            "daemon_acknowledged": (
                "degraded"
                if daemon.receiver.acknowledgements_failed
                else "issued"
                if daemon.receiver.acknowledgements_sent
                else "waiting"
            ),
            "chain_continuity": (
                "error" if daemon.receiver.hash_chain_break_count else
                "checked" if daemon._buffer_hash_count else "waiting"
            ),
            "export_detected": "complete" if daemon._last_export_path else "waiting",
            "audio_association": "evaluated" if daemon._last_manifest_path else "waiting",
            "evidence_sealed": "complete" if daemon._last_manifest_path else "waiting",
            "verification": daemon._last_verifier_outcome or "waiting",
        },
        "daemon_receipt_acknowledgement": daemon.receiver.receipt_summary(),
        "links": {
            "manifest": daemon._status_link(daemon._last_manifest_path),
            "fight_card": daemon._status_link(daemon._last_report_path),
            "verifier_result": daemon._status_link(daemon._last_verification_path),
            "downstream_handoff": daemon._status_link(daemon._last_handoff_path),
            "bundle_index": daemon._status_link(daemon._last_bundle_index_path),
            "evidence_bundle": daemon._status_link(daemon._last_bundle_path),
        },
        "proof_levels": [
            "directly_observed", "inferred", "user_declared",
            "externally_verified", "unknown_unobserved",
        ],
    }
    # Unique temp per writer thread: the 1s loop and the export watcher both
    # call this, and a shared .tmp lets their write/replace pairs interleave.
    temporary = daemon._status_path.with_suffix(f".tmp-{threading.get_ident()}")
    temporary.write_text(json.dumps(data, separators=(",", ":")), encoding="utf-8")
    temporary.replace(daemon._status_path)
    write_dashboard(data, daemon._status_path.parent / "dashboard.html")
