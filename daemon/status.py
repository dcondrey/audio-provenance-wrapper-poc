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

def derive_readiness(daemon: "Daemon", state: str | None = None) -> dict[str, object]:
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
    if telemetry.get("bypassed_buffers", 0):
        alerts.append(
            "The host invoked the plug-in bypass path; those buffers passed through unchanged "
            "but were intentionally not claimed as observed, so coverage will grade partial."
        )
    if daemon._last_manifest_error:
        alerts.append(
            f"Sealing the last export failed ({daemon._last_manifest_error}). No manifest, "
            "fight card or evidence bundle was written for it; check the daemon log."
        )
    window_size = int(last_hash.get("window_size_samples") or 0)
    sample_rate = int(last_hash.get("sample_rate_hz") or 0)
    routed_seconds = (
        chain_length * window_size / sample_rate if window_size and sample_rate else 0.0
    )
    ended = state == "stopped"
    if ended:
        min_export_seconds = None
        export_guidance = (
            "This capture session has ended and its manifest is sealed. Start a new session "
            "before exporting again; anything rendered now is outside the observed window."
        )
    elif routed_seconds:
        min_export_seconds = max(1, int(routed_seconds * MIN_ROUTED_COVERAGE + 0.999))
        export_guidance = (
            f"Render at least {min_export_seconds}s (routed audio observed so far: "
            f"{routed_seconds:.1f}s). A shorter export dilutes the association "
            f"below the {int(MIN_ROUTED_COVERAGE * 100)}% overlap floor."
        )
    else:
        min_export_seconds = None
        export_guidance = "Waiting for routed audio before export guidance is available."
    return {
        "ready_to_export": bool(chain_length) and not alerts and not ended,
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
    c2pa_claim: dict[str, object] | None = None,
    chain_root: str | None = None,
    chain_length: int = 0,
) -> dict[str, object]:
    """Build the registrar handoff from the caller's locked snapshot.

    IMPORTANT: chain_root and chain_length are passed in rather than re-read from
    the daemon. The generator hashes the bound evidence prefix, signs a C2PA
    claim and re-verifies it before reaching this point, and a buffer_hash
    arriving in that window used to commit a chain root newer than the evidence
    the manifest binds, signed and with nothing checking it.
    """
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
            "hash_chain_root": chain_root,
            "hash_chain_length": chain_length,
            "apw:proof_level": "directly_observed" if chain_length else "unknown_unobserved",
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
        "c2pa_claim": _handoff_c2pa_claim(c2pa_claim),
        "descriptive_c2pa_assertion_mapping": {
            "status": "descriptive_projection_not_the_signed_claim",
            "assertions": ["c2pa.hash.data", "c2pa.ingredient", "c2pa.actions", "apw.unobserved"],
            "apw:proof_level": "inferred",
        },
        "missing_downstream_requirements": [
            "verified creator or institution identity",
            "author-controlled credential and key policy",
            "production certificate chain issued by a recognised authority",
            "publication on a recognised C2PA trust list",
            "audio-native soft binding or watermark",
            "resilient recovery from the audio",
            "registry publication",
            "consent and rights verification",
        ],
        "boundary": (
            "This neutral handoff is not a provider-specific API payload and claims no compatibility "
            "with proprietary watermark, recovery, identity, signing, or registry technology."
        ),
    }


def _handoff_c2pa_claim(claim: dict[str, object] | None) -> dict[str, object]:
    """Summarise the signed claim for a downstream registrar.

    The full claim lives in the manifest; the handoff carries only what a
    registrar needs to locate and re-check it, plus the trust caveat.
    """
    if not isinstance(claim, dict) or claim.get("status") in {None, "unavailable"}:
        return {
            "status": "unavailable",
            "reason": (claim or {}).get("reason", "No C2PA claim was produced for this export."),
            "apw:proof_level": "unknown_unobserved",
        }
    validation = claim.get("validation") if isinstance(claim.get("validation"), dict) else {}
    asset = claim.get("signed_asset") if isinstance(claim.get("signed_asset"), dict) else {}
    signer = claim.get("signer") if isinstance(claim.get("signer"), dict) else {}
    return {
        "status": claim.get("status"),
        "signed_asset": asset.get("relative_path"),
        "signed_asset_sha256": asset.get("sha256"),
        "sidecar_manifest": claim.get("sidecar_manifest"),
        "hard_binding_algorithm": (claim.get("hard_binding") or {}).get("algorithm"),
        "validation_state": validation.get("state"),
        "trust_anchor_scope": validation.get("trust_anchor_scope"),
        "signer_key_id": signer.get("key_id"),
        "signer_identity": "not_established",
        "boundary": (
            "The claim is signed with a certificate chain this machine issued to itself. "
            "A downstream registrar must re-issue under its own credential policy before "
            "the claim carries any identity meaning."
        ),
        "apw:proof_level": "directly_observed",
    }


def status_link(path: Path | None, status_dir: Path) -> str | None:
    """Dashboard-relative link; relative_to raises when --manifest-dir and the
    status dir do not share a root, which killed the export-watcher thread."""
    if path is None:
        return None
    return os.path.relpath(path, start=status_dir)


def write_status(daemon: "Daemon", state: str) -> None:
    if state != "stopped" and (
        daemon.receiver.rejected_count > 0
        or daemon.receiver.hash_chain_break_count > 0
        or daemon._last_manifest_error is not None
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
            "Verified, changed, untrusted, incomplete, and not_found are local POC integrity outcomes, not registry outcomes."
        ),
        "coverage": daemon._derive_coverage(daemon._buffer_hash_count),
        "readiness": daemon._derive_readiness(state),
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
            # The real association result, not the mere existence of a manifest:
            # a 32-bit float export produced neither an association nor a C2PA
            # claim while this card stayed green.
            "audio_association": daemon._last_association_status or "waiting",
            "evidence_sealed": (
                "error" if daemon._last_manifest_error
                else "complete" if daemon._last_manifest_path
                else "waiting"
            ),
            "verification": daemon._last_verifier_outcome or "waiting",
            "sdk_adapter": (
                "error" if daemon._last_sdk_error
                else daemon._last_sdk_verification_status or "waiting"
            ),
        },
        "daemon_receipt_acknowledgement": daemon.receiver.receipt_summary(),
        "links": {
            "manifest": daemon._status_link(daemon._last_manifest_path),
            "fight_card": daemon._status_link(daemon._last_report_path),
            "verifier_result": daemon._status_link(daemon._last_verification_path),
            "downstream_handoff": daemon._status_link(daemon._last_handoff_path),
            "bundle_index": daemon._status_link(daemon._last_bundle_index_path),
            "evidence_bundle": daemon._status_link(daemon._last_bundle_path),
            "sdk_adapter_receipt": daemon._status_link(daemon._last_sdk_receipt_path),
        },
        "sdk_adapter": {
            "enabled": daemon.sdk_adapter_enabled,
            "development_only": True if daemon.sdk_adapter_enabled else None,
            "identity": "not_established" if daemon.sdk_adapter_enabled else None,
            "record_id": daemon._last_sdk_record_id,
            "verification_status": daemon._last_sdk_verification_status,
            "error": daemon._last_sdk_error,
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
