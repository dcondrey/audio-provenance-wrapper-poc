from __future__ import annotations

import argparse
import hashlib
import json
import logging
import signal
import socket
import subprocess
import threading
import time
import uuid
from collections import deque
from pathlib import Path

from daemon.audio_association import associate_export
from daemon.bundle import create_evidence_bundle
from daemon.common import append_jsonl, sha256_file, sha256_prefix, utc_timestamp
from daemon.correlation_engine.engine import CorrelationEngine, LayerEvent
from daemon.dashboard import write_dashboard
from daemon.evidence_receiver.receiver import EvidenceReceiver
from daemon.hardware_attestation.provider import HardwareProvider, SoftwareProvider, detect_provider
from daemon.manifest_builder.builder import (
    ExportEvidence,
    IngredientEvidence,
    ManifestBuilder,
    StemEvidence,
)
from daemon.report import write_html_report
from daemon.sample_watcher.watcher import SampleWatcher, extract_audio_metadata
from daemon.signing import DEFAULT_PRIVATE_KEY, DEFAULT_PUBLIC_KEY, Ed25519Signer

log = logging.getLogger(__name__)

_LAYER_MAP: dict[str, str] = {
    "buffer_hash": "audio_buffer",
    "audio_transition": "audio_buffer",
    "spectral_shift": "audio_buffer",
    "spectral_profile_change": "audio_buffer",
    "transport_change": "transport",
    "midi_event": "midi",
    "parameter_change": "midi",
    "session_config_change": "session",
}


def _event_type_to_layer(event_type: str) -> str:
    return _LAYER_MAP.get(event_type, "audio_buffer")


DEFAULT_UDP_PORT = 9876
DEFAULT_EVIDENCE_DIR = Path("evidence")
DEFAULT_SAMPLE_DIR = Path("~/Music/ProvenanceSamples")
DEFAULT_MANIFEST_DIR = Path("manifests")
DEFAULT_SIGNING_KEY = Path("~/.apw/demo_signing_key.bin")
SOURCE_CATEGORIES = (
    "unknown",
    "audio_interface_recording",
    "midi_vst_synth",
    "imported_sample",
    "generator",
    "resampling",
    "manual_import",
)


class Daemon:
    """Unified daemon that orchestrates all observation layers."""

    def __init__(
        self,
        udp_port: int = DEFAULT_UDP_PORT,
        evidence_dir: Path = DEFAULT_EVIDENCE_DIR,
        sample_dir: Path = DEFAULT_SAMPLE_DIR,
        project_path: Path | None = None,
        export_dir: Path | None = None,
        manifest_dir: Path = DEFAULT_MANIFEST_DIR,
        session_id: str | None = None,
        stem_id: str = "stem-1",
        source_category: str = "unknown",
        signing_key_path: Path = DEFAULT_SIGNING_KEY,
        portable_private_key_path: Path = DEFAULT_PRIVATE_KEY,
        portable_public_key_path: Path = DEFAULT_PUBLIC_KEY,
        hardware_provider: HardwareProvider | None = None,
        generate_html_report: bool = True,
        open_artifacts: bool = False,
    ) -> None:
        if source_category not in SOURCE_CATEGORIES:
            raise ValueError(f"Unsupported source category: {source_category}")

        self.evidence_dir = evidence_dir.expanduser()
        self.manifest_dir = manifest_dir.expanduser()
        self.evidence_dir.mkdir(parents=True, exist_ok=True)
        self.manifest_dir.mkdir(parents=True, exist_ok=True)

        self.session_id = session_id or (
            f"capture-{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{uuid.uuid4().hex[:8]}"
        )
        self.stem_id = stem_id
        self.source_category = source_category
        self.source_category_proof_level = (
            "unknown_unobserved" if source_category == "unknown" else "user_declared"
        )
        self.generate_html_report = generate_html_report
        self.open_artifacts = open_artifacts

        self.receiver = EvidenceReceiver(
            host="127.0.0.1",
            port=udp_port,
            evidence_path=self.evidence_dir / "plugin_events.jsonl",
            capture_session_id=self.session_id,
            stem_id=self.stem_id,
        )

        self.correlation = CorrelationEngine(
            window_ms=2000,
            evidence_path=self.evidence_dir / "composite_events.jsonl",
        )

        self.sample_watcher = SampleWatcher(
            watch_dir=sample_dir,
            evidence_path=self.evidence_dir / "sample_import_events.jsonl",
            poll_interval_seconds=2.0,
        )

        self.project_path = project_path
        self.export_dir = export_dir.expanduser() if export_dir else None
        if self.export_dir is not None:
            self.export_dir.mkdir(parents=True, exist_ok=True)
        self._export_seen: dict[str, tuple[int, int]] = {}
        self._session_lock = threading.Lock()
        self._session_events: deque[dict[str, object]] = deque()
        self._max_session_events = 50_000
        self._session_event_drops = 0
        self._feature_events: deque[dict[str, object]] = deque(maxlen=12_000)
        self._feature_window_drops = 0
        self._buffer_hash_count = 0
        self._first_hash_event: dict[str, object] | None = None
        self._last_hash_event: dict[str, object] | None = None
        self._plugin_instance_ids: set[str] = set()
        self._latest_plugin_telemetry: dict[str, int] = {}
        self._export_versions: dict[str, int] = {}
        self._last_manifest_path: Path | None = None
        self._last_report_path: Path | None = None
        self._last_verification_path: Path | None = None
        self._last_handoff_path: Path | None = None
        self._last_bundle_index_path: Path | None = None
        self._last_bundle_path: Path | None = None
        self._last_verifier_outcome: str | None = None
        self._last_export_path: Path | None = None
        self._status_path = self.evidence_dir.parent / "status.json"
        self._session_started_at = utc_timestamp()
        self._active_layers: set[str] = {"sample_watcher"}
        self._latest_project_snapshot = None
        self._plugin_seen = False
        self._last_plugin_event_monotonic = 0.0
        self._stop = threading.Event()
        self._portable_signer = Ed25519Signer(
            portable_private_key_path,
            portable_public_key_path,
        )

        if hardware_provider is not None:
            self._hw_provider = hardware_provider
        else:
            try:
                self._hw_provider = detect_provider(signing_key_path)
            except Exception:
                log.warning("Signer detection failed; using software fallback", exc_info=True)
                self._hw_provider = SoftwareProvider(key_path=signing_key_path)

    def _append_event(self, event: dict[str, object]) -> None:
        with self._session_lock:
            if len(self._session_events) >= self._max_session_events:
                self._session_events.popleft()
                self._session_event_drops += 1
                if self._session_event_drops == 1 or self._session_event_drops % 10_000 == 0:
                    log.warning(
                        "Session event memory limit reached; dropped=%d max=%d",
                        self._session_event_drops,
                        self._max_session_events,
                    )
            self._session_events.append(event)

    def _record_plugin_event(self, event: dict[str, object], layer: str) -> None:
        self._append_event(event)
        with self._session_lock:
            self._active_layers.add(layer)
            instance_id = str(event.get("plugin_instance_id", "unknown_plugin_instance"))
            self._plugin_instance_ids.add(instance_id)
            telemetry = event.get("telemetry")
            if isinstance(telemetry, dict):
                for key, value in telemetry.items():
                    if isinstance(value, int):
                        self._latest_plugin_telemetry[str(key)] = value
            if event.get("event_type") == "buffer_hash":
                self._buffer_hash_count += 1
                if self._first_hash_event is None:
                    self._first_hash_event = dict(event)
                self._last_hash_event = dict(event)
                if len(self._feature_events) == self._feature_events.maxlen:
                    self._feature_window_drops += 1
                self._feature_events.append(dict(event))

    def _correlate(self, layer_event: LayerEvent) -> None:
        try:
            composites = self.correlation.ingest(layer_event)
        except Exception:
            log.exception("Correlation engine error")
            return
        for c in composites:
            log.info("Composite edit: %s (%.2f)", c.edit_type, c.confidence)
            self._append_event(c.to_event_dict())

    def run(self) -> None:
        threads: list[threading.Thread] = [
            threading.Thread(target=self._run_udp_receiver, name="udp-receiver", daemon=True),
            threading.Thread(target=self._run_sample_watcher, name="sample-watcher", daemon=True),
        ]

        if self.project_path and self.project_path.exists():
            threads.append(
                threading.Thread(target=self._run_project_watcher, name="project-watcher", daemon=True)
            )
        elif self.project_path:
            log.warning("Project watcher disabled because the file does not exist: %s", self.project_path)

        if self.export_dir:
            threads.append(
                threading.Thread(target=self._run_export_watcher, name="export-watcher", daemon=True)
            )

        log.info("Capture session: %s", self.session_id)
        log.info("Declared source category: %s (%s)", self.source_category, self.source_category_proof_level)
        log.info("Daemon starting with %d threads", len(threads))
        self._write_evidence("session_events.jsonl", {
            "event_type": "session_start",
            "capture_session_id": self.session_id,
            "stem_id": self.stem_id,
            "started_at": self._session_started_at,
            "daemon_monotonic_ms": int(time.monotonic_ns() // 1_000_000),
            "proof_level": "directly_observed",
        })
        self._write_status("idle")
        for t in threads:
            t.start()

        try:
            while not self._stop.is_set():
                self._stop.wait(1.0)
                recently_active = (
                    self._plugin_seen
                    and time.monotonic() - self._last_plugin_event_monotonic < 3.0
                )
                self._write_status("active" if recently_active else "idle")
        except KeyboardInterrupt:
            log.info("Shutting down")
            self._stop.set()
        finally:
            self._stop.set()
            self.receiver.close()
            self._write_evidence("session_events.jsonl", {
                "event_type": "session_end",
                "capture_session_id": self.session_id,
                "ended_at": utc_timestamp(),
                "daemon_monotonic_ms": int(time.monotonic_ns() // 1_000_000),
                "proof_level": "directly_observed",
                "diagnostics": self.receiver.diagnostics(),
            })
            self._write_status("stopped")

    def stop(self) -> None:
        self._stop.set()
        self.receiver.close()

    def _run_udp_receiver(self) -> None:
        log.info("UDP receiver on port %d", self.receiver.port)
        self.receiver.sock.settimeout(1.0)

        while not self._stop.is_set():
            try:
                data, address = self.receiver.sock.recvfrom(65535)
            except socket.timeout:
                continue
            except OSError:
                if self._stop.is_set():
                    break
                log.exception("UDP socket error")
                continue

            event, acknowledgement = self.receiver.process_packet_with_ack(data)
            if not self.receiver.send_acknowledgement(address, acknowledgement):
                log.warning(
                    "Could not dispatch local daemon acknowledgement to %s:%d",
                    address[0],
                    address[1],
                )
            if event is None:
                continue
            self._last_plugin_event_monotonic = time.monotonic()

            et = str(event.get("event_type", ""))
            layer = _event_type_to_layer(et)
            self._record_plugin_event(event, layer)
            if not self._plugin_seen:
                self._plugin_seen = True
                log.info("Capture plugin evidence stream detected")
            layer_event = LayerEvent(
                layer=layer,
                event_type=et,
                timestamp_ms=int(event.get("daemon_received_monotonic_ms", 0)),
                data=event,
            )
            self._correlate(layer_event)

    def _run_sample_watcher(self) -> None:
        self.sample_watcher.mark_existing_seen()
        log.info("Sample watcher on %s", self.sample_watcher.watch_dir)

        while not self._stop.is_set():
            try:
                events = self.sample_watcher.scan_once()
            except Exception:
                log.exception("Sample watcher error")
                time.sleep(self.sample_watcher.poll_interval_seconds)
                continue

            for event in events:
                log.info("Sample detected: %s", event.get("file_name"))
                event["source_timestamp"] = event.get("observed_at")
                event["daemon_observed_monotonic_ms"] = int(time.monotonic_ns() // 1_000_000)
                self._append_event(event)

                layer_event = LayerEvent(
                    layer="sample_watcher",
                    event_type="sample_file_observed",
                    timestamp_ms=int(event["daemon_observed_monotonic_ms"]),
                    data=event,
                )
                self._correlate(layer_event)

            time.sleep(self.sample_watcher.poll_interval_seconds)

    def _run_project_watcher(self) -> None:
        from daemon.project_differ.differ import extract_snapshot, compute_diff

        log.info("Project watcher on %s", self.project_path)
        prev_snapshot = None
        prev_mtime_ns = 0

        while not self._stop.is_set():
            try:
                stat = self.project_path.stat()
            except OSError:
                time.sleep(2.0)
                continue

            if stat.st_mtime_ns != prev_mtime_ns:
                prev_mtime_ns = stat.st_mtime_ns
                try:
                    snapshot = extract_snapshot(self.project_path)
                except Exception:
                    log.exception("Failed to parse %s", self.project_path)
                    time.sleep(2.0)
                    continue

                with self._session_lock:
                    self._active_layers.add("project_differ")

                if prev_snapshot is not None:
                    diff = compute_diff(prev_snapshot, snapshot)
                    if diff.has_changes():
                        diff_event: dict[str, object] = {
                            "event_type": "project_diff",
                            "proof_level": "inferred",
                            "source_timestamp_ms": int(time.time() * 1000),
                            "timestamp_ms": int(time.time() * 1000),
                            "daemon_observed_monotonic_ms": int(time.monotonic_ns() // 1_000_000),
                            "clips_added": diff.clips_added,
                            "clips_removed": diff.clips_removed,
                            "clips_modified": diff.clips_modified,
                            "tracks_added": diff.tracks_added,
                            "tracks_removed": diff.tracks_removed,
                            "devices_changed": diff.devices_changed,
                            "samples_added": sorted(diff.samples_added),
                            "samples_removed": sorted(diff.samples_removed),
                            "midi_notes_delta": diff.midi_notes_delta,
                            "automation_points_delta": diff.automation_points_delta,
                            "bpm_changed": diff.bpm_changed,
                        }
                        self._write_evidence("project_diff_events.jsonl", diff_event)
                        self._append_event(diff_event)

                        layer_event = LayerEvent(
                            layer="project_differ",
                            event_type="project_diff",
                            timestamp_ms=int(diff_event["daemon_observed_monotonic_ms"]),
                            data=diff_event,
                        )
                        self._correlate(layer_event)
                        log.info(
                            "Project diff: +%d/-%d/~%d clips",
                            diff.clips_added, diff.clips_removed, diff.clips_modified,
                        )

                prev_snapshot = snapshot
                self._latest_project_snapshot = snapshot

            time.sleep(2.0)

    def _run_export_watcher(self) -> None:
        log.info("Export watcher on %s", self.export_dir)
        AUDIO_EXTENSIONS = {".wav", ".aiff", ".aif"}

        for existing in self.export_dir.iterdir():
            if existing.is_file() and existing.suffix.lower() in AUDIO_EXTENSIONS:
                try:
                    self._export_seen[str(existing.resolve())] = self._export_signature(existing)
                except OSError:
                    continue

        while not self._stop.is_set():
            try:
                for path in self.export_dir.iterdir():
                    if not path.is_file():
                        continue
                    if path.suffix.lower() not in AUDIO_EXTENSIONS:
                        continue

                    resolved = str(path.resolve())
                    try:
                        signature = self._export_signature(path)
                    except OSError:
                        continue
                    if self._export_seen.get(resolved) == signature:
                        continue

                    if not self._file_is_stable(path):
                        continue

                    try:
                        log.info("Export detected: %s", path.name)
                        version = self._export_versions.get(resolved, 0) + 1
                        self._generate_manifest(path, export_version=version)
                        self._export_versions[resolved] = version
                        self._export_seen[resolved] = self._export_signature(path)
                    except Exception:
                        log.exception("Manifest generation failed for %s; will retry", path)
            except OSError:
                log.exception("Export watcher could not scan %s", self.export_dir)

            time.sleep(2.0)

    def _generate_manifest(self, export_path: Path, export_version: int = 1) -> Path:
        self._last_export_path = export_path
        export_hash = sha256_file(export_path)
        stat = export_path.stat()
        export_metadata = extract_audio_metadata(export_path)
        builder = ManifestBuilder(
            session_id=self.session_id,
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

        with self._session_lock:
            events_snapshot = list(self._session_events)
            feature_snapshot = list(self._feature_events)
            first_hash_event = dict(self._first_hash_event or {})
            last_hash_event = dict(self._last_hash_event or {})
            chain_length = self._buffer_hash_count
            plugin_instance_ids = tuple(sorted(self._plugin_instance_ids))

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
                    proof_level=str(event.get("proof_level", "directly_observed")),
                    correlation_confidence=None,
                    audio_fingerprint=event.get("audio_fingerprint"),
                ))
            elif et == "composite_edit":
                builder.add_composite_edit(event)

        if chain_length > 0:
            builder.add_stem(StemEvidence(
                stem_id=self.stem_id,
                hash_chain_root=last_window_hash,
                hash_chain_length=chain_length,
                first_observed_ms=first_hash_ms,
                last_observed_ms=last_hash_ms,
                sample_rate_hz=stem_sr,
                channel_count=stem_ch,
                source_category=self.source_category,
                proof_level="directly_observed",
                source_category_proof_level=self.source_category_proof_level,
                hash_chain_genesis=chain_genesis,
                first_received_at=first_received_at,
                last_received_at=last_received_at,
                plugin_instance_ids=plugin_instance_ids,
            ))

        builder.coverage = self._derive_coverage(chain_length)
        association = associate_export(export_path, feature_snapshot)
        association.update({
            "capture_session_id": self.session_id,
            "stem_ids": [self.stem_id] if chain_length else [],
            "export_file_name": export_path.name,
            "basis": (
                "A bounded sequence of relative RMS, zero-crossing, crest-factor, and coarse energy-envelope "
                "features emitted from accepted routed plug-in windows was compared with equivalent streaming-"
                "extracted export features using time-offset search. The relationship remains inferred and does "
                "not establish complete routing."
            ),
        })
        builder.audio_association = association
        builder.session_diagnostics = self._session_diagnostics()

        all_layers = {"audio_buffer", "transport", "midi", "session",
                      "sample_watcher", "project_differ", "input_capture",
                      "screen_observer"}
        with self._session_lock:
            active_layers = set(self._active_layers)
        missing_layers = sorted(all_layers - active_layers)
        builder.unobserved = list(builder.unobserved)
        for layer in missing_layers:
            builder.unobserved.append(f"layer_{layer}_not_active")

        evidence_hashes: dict[str, str] = {}
        evidence_files: dict[str, dict[str, object]] = {}
        for evidence_file in sorted(self.evidence_dir.glob("*.jsonl")):
            byte_length = evidence_file.stat().st_size
            digest = sha256_prefix(evidence_file, byte_length)
            evidence_hashes[evidence_file.name] = digest
            evidence_files[evidence_file.name] = {
                "sha256": digest,
                "byte_length": byte_length,
                "binding_scope": "file_prefix_at_manifest_creation",
            }

        manifest = builder.build()

        snap = self._latest_project_snapshot
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
            "evidence_directory": str(self.evidence_dir.resolve()),
            "evidence_file_hashes": evidence_hashes,
            "evidence_files": evidence_files,
            "last_window_hash": last_window_hash,
            "chain_length": chain_length,
            "apw:proof_level": "directly_observed",
        }
        manifest["capture_session"]["started_at"] = self._session_started_at
        manifest["capture_session"]["state_at_manifest"] = "active"
        receipt_summary = self.receiver.receipt_summary()
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
        manifest_path = self.manifest_dir / f"{export_path.stem}{suffix}_manifest.json"
        report_path = self.manifest_dir / f"{export_path.stem}{suffix}_provenance.html"
        artifact_dir = self.manifest_dir / "artifacts"
        artifact_dir.mkdir(parents=True, exist_ok=True)
        verification_path = artifact_dir / f"{export_path.stem}{suffix}_verification.json"
        handoff_path = artifact_dir / f"{export_path.stem}{suffix}_handoff.json"
        bundle_index_path = artifact_dir / f"{export_path.stem}{suffix}_bundle_index.json"
        bundle_path = artifact_dir / f"{export_path.stem}{suffix}_evidence_bundle.zip"
        manifest["presentation"] = {
            "html_report": report_path.name if self.generate_html_report else None,
            "derived_from": manifest_path.name,
            "verifier_result": str(verification_path.relative_to(self.manifest_dir)),
            "downstream_handoff": str(handoff_path.relative_to(self.manifest_dir)),
            "bundle_index": (
                str(bundle_index_path.relative_to(self.manifest_dir))
                if self.generate_html_report else None
            ),
            "evidence_bundle": (
                str(bundle_path.relative_to(self.manifest_dir))
                if self.generate_html_report else None
            ),
            "apw:proof_level": "directly_observed",
        }
        manifest["downstream_registration_handoff"] = self._build_handoff(
            export_hash=export_hash,
            association=association,
            coverage=builder.coverage,
            evidence_files=evidence_files,
            manifest_name=manifest_path.name,
            bundle_name=bundle_path.name if self.generate_html_report else None,
            bundle_index_name=bundle_index_path.name if self.generate_html_report else None,
        )

        try:
            manifest["portable_signature"] = self._portable_signer.sign_manifest(manifest)
        except Exception:
            log.exception("Could not create portable Ed25519 signature")

        try:
            identity = self._hw_provider.device_identity()
            manifest_bytes = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
            signature = self._hw_provider.sign(manifest_bytes)
            manifest["manifest_signature"] = {
                "algorithm": identity.algorithm,
                "device_id": identity.device_id,
                "public_key_hex": identity.public_key_hex,
                "signature_hex": signature.hex(),
                "signed_content_hash": hashlib.sha256(manifest_bytes).hexdigest(),
                "trust_scope": (
                    "local_software_integrity"
                    if isinstance(self._hw_provider, SoftwareProvider)
                    else "hardware_provider"
                ),
                "hardware_attested": not isinstance(self._hw_provider, SoftwareProvider),
                "apw:proof_level": (
                    "unknown_unobserved"
                    if isinstance(self._hw_provider, SoftwareProvider)
                    else "directly_observed"
                ),
                "notes": (
                    "The local HMAC seal detects changes when checked with the same secret key; "
                    "it is not hardware attestation or third-party identity verification."
                    if isinstance(self._hw_provider, SoftwareProvider)
                    else "Signature produced by the configured hardware provider."
                ),
            }
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
        if self.generate_html_report:
            write_html_report(dict(manifest), report_path)
        verification = verify_manifest(manifest_path)
        verification_path.write_text(
            json.dumps(verification.to_dict(), indent=2, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        if self.generate_html_report:
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
                    signer=self._portable_signer,
                )
                log.info("Evidence bundle written: %s", bundle_path)
            except Exception:
                log.exception("Could not create deterministic evidence bundle")
            if self.open_artifacts:
                try:
                    subprocess.Popen(["open", str(report_path)])
                except OSError:
                    log.warning("Could not open fight card automatically", exc_info=True)
        self._last_verifier_outcome = verification.outcome
        self._last_manifest_path = manifest_path
        self._last_report_path = report_path if self.generate_html_report else None
        self._last_verification_path = verification_path
        self._last_handoff_path = handoff_path
        self._last_bundle_index_path = bundle_index_path if bundle_index_path.is_file() else None
        self._last_bundle_path = bundle_path if bundle_path.is_file() else None
        self._last_export_path = export_path
        self._write_status(
            "active"
            if self._plugin_seen and time.monotonic() - self._last_plugin_event_monotonic < 3.0
            else "idle"
        )
        if chain_length == 0:
            log.warning("Export was hashed, but no routed-audio hash events were received")
        return manifest_path

    def _derive_coverage(self, chain_length: int) -> dict[str, object]:
        receiver = self.receiver.diagnostics()
        with self._session_lock:
            telemetry = dict(self._latest_plugin_telemetry)
            plugin_instance_count = len(self._plugin_instance_ids)
        counters: dict[str, int] = {
            **telemetry,
            **receiver,
            "udp_sends_locally_emitted": max(
                0,
                telemetry.get("udp_sends_attempted", 0) - telemetry.get("udp_sends_failed", 0),
            ),
            "buffer_hash_events_received": chain_length,
            "feature_windows_dropped_from_alignment_buffer": self._feature_window_drops,
        }
        windows_hashed = telemetry.get("windows_hashed")
        required = {
            "buffers_submitted",
            "samples_submitted",
            "windows_hashed",
            "fifo_samples_dropped",
            "fifo_windows_dropped",
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
            telemetry.get("udp_sends_failed", 0),
            receiver["sequence_gaps"],
            receiver["sequence_out_of_order"],
            receiver["hash_chain_breaks"],
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

    def _session_diagnostics(self) -> dict[str, object]:
        return {
            "receiver": self.receiver.diagnostics(),
            "correlation": {
                "buffer_events": self.correlation.buffer_size,
                "buffer_max_events": self.correlation.max_buffer_events,
                "capacity_drops": self.correlation.capacity_drops,
                "duplicate_matches_suppressed": self.correlation.duplicate_suppressions,
                "composite_events_emitted": self.correlation.emitted_count,
                "clock": "daemon_monotonic_ms",
            },
            "session_memory": {
                "events_retained": len(self._session_events),
                "max_events": self._max_session_events,
                "events_dropped_from_memory_only": self._session_event_drops,
            },
            "daemon_receipt_acknowledgement": self.receiver.receipt_summary(),
            "apw:proof_level": "directly_observed",
        }

    def _build_handoff(
        self,
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
            "capture_session_id": self.session_id,
            "export_hard_hash": {
                "algorithm": "sha256",
                "value": export_hash,
                "apw:proof_level": "directly_observed",
            },
            "routed_observation_commitment": {
                "hash_chain_root": str((self._last_hash_event or {}).get("window_hash", "")) or None,
                "hash_chain_length": self._buffer_hash_count,
                "apw:proof_level": (
                    "directly_observed" if self._buffer_hash_count else "unknown_unobserved"
                ),
            },
            "coverage": coverage,
            "audio_association": association,
            "creator_declarations": [{
                "name": "source_category",
                "value": self.source_category,
                "apw:proof_level": self.source_category_proof_level,
            }],
            "signing_key": {
                "algorithm": "Ed25519",
                "public_key_hex": self._portable_signer.public_key_hex(),
                "public_key_file": str(self._portable_signer.public_key_path),
                "trust_scope": "self_generated_demo_key_integrity",
                "signer_identity": "not_established",
                "apw:proof_level": "unknown_unobserved",
            },
            "evidence_bundle": {
                "manifest": str((self.manifest_dir / manifest_name).resolve()),
                "export": str(self._last_export_path.resolve()) if self._last_export_path else None,
                "evidence_directory": str(self.evidence_dir.resolve()),
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

    def _write_status(self, state: str) -> None:
        if state != "stopped" and (
            self.receiver.rejected_count > 0 or self.receiver.hash_chain_break_count > 0
        ):
            state = "error"
        with self._session_lock:
            plugin_instance_ids = sorted(self._plugin_instance_ids)
            plugin_telemetry = dict(self._latest_plugin_telemetry)
        data = {
            "product": "Routed Audio Evidence Adapter",
            "state": state,
            "updated_at": utc_timestamp(),
            "session_id": self.session_id,
            "stem_id": self.stem_id,
            "plugin_instance_ids": plugin_instance_ids,
            "trust_boundary": (
                "Only routed plug-in audio and local filesystem exports are observed. "
                "Identity, authorship, rights, consent, bypassed paths, and downstream registration remain unestablished. "
                "Verified, changed, untrusted, and not_found are local POC integrity outcomes, not registry outcomes."
            ),
            "coverage": self._derive_coverage(self._buffer_hash_count),
            "counts": {
                **plugin_telemetry,
                **self.receiver.diagnostics(),
                "udp_sends_locally_emitted": max(
                    0,
                    plugin_telemetry.get("udp_sends_attempted", 0)
                    - plugin_telemetry.get("udp_sends_failed", 0),
                ),
                "buffer_hash_events_received": self._buffer_hash_count,
            },
            "pipeline": {
                "plugin_observed": "complete" if self._buffer_hash_count else "waiting",
                "plugin_emitted": (
                    "degraded"
                    if plugin_telemetry.get("udp_sends_failed", 0)
                    else "complete"
                    if plugin_telemetry.get("udp_sends_attempted", 0)
                    else "waiting"
                ),
                "daemon_received": "complete" if self.receiver.event_count else "waiting",
                "daemon_acknowledged": (
                    "degraded"
                    if self.receiver.acknowledgements_failed
                    else "issued"
                    if self.receiver.acknowledgements_sent
                    else "waiting"
                ),
                "chain_continuity": (
                    "error" if self.receiver.hash_chain_break_count else
                    "checked" if self._buffer_hash_count else "waiting"
                ),
                "export_detected": "complete" if self._last_export_path else "waiting",
                "audio_association": "evaluated" if self._last_manifest_path else "waiting",
                "evidence_sealed": "complete" if self._last_manifest_path else "waiting",
                "verification": self._last_verifier_outcome or "waiting",
            },
            "daemon_receipt_acknowledgement": self.receiver.receipt_summary(),
            "links": {
                "manifest": (
                    str(self._last_manifest_path.relative_to(self._status_path.parent))
                    if self._last_manifest_path else None
                ),
                "fight_card": (
                    str(self._last_report_path.relative_to(self._status_path.parent))
                    if self._last_report_path else None
                ),
                "verifier_result": (
                    str(self._last_verification_path.relative_to(self._status_path.parent))
                    if self._last_verification_path else None
                ),
                "downstream_handoff": (
                    str(self._last_handoff_path.relative_to(self._status_path.parent))
                    if self._last_handoff_path else None
                ),
                "bundle_index": (
                    str(self._last_bundle_index_path.relative_to(self._status_path.parent))
                    if self._last_bundle_index_path else None
                ),
                "evidence_bundle": (
                    str(self._last_bundle_path.relative_to(self._status_path.parent))
                    if self._last_bundle_path else None
                ),
            },
            "proof_levels": [
                "directly_observed", "inferred", "user_declared",
                "externally_verified", "unknown_unobserved",
            ],
        }
        temporary = self._status_path.with_suffix(".tmp")
        temporary.write_text(json.dumps(data, separators=(",", ":")), encoding="utf-8")
        temporary.replace(self._status_path)
        write_dashboard(data, self._status_path.parent / "dashboard.html")

    @staticmethod
    def _file_is_stable(path: Path, checks: int = 3, interval: float = 0.5) -> bool:
        try:
            previous = Daemon._export_signature(path)
        except OSError:
            return False
        for _ in range(checks):
            time.sleep(interval)
            try:
                current = Daemon._export_signature(path)
            except OSError:
                return False
            if current != previous:
                return False
            previous = current
        return previous[0] > 0

    @staticmethod
    def _export_signature(path: Path) -> tuple[int, int]:
        stat = path.stat()
        return stat.st_size, stat.st_mtime_ns

    def _write_evidence(self, filename: str, event: dict[str, object]) -> None:
        append_jsonl(self.evidence_dir / filename, event)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Audio provenance daemon: receives plugin events, watches files, generates manifests.",
    )
    parser.add_argument("--port", type=int, default=DEFAULT_UDP_PORT, help="UDP port for plugin events.")
    parser.add_argument("--evidence-dir", type=Path, default=DEFAULT_EVIDENCE_DIR, help="Evidence output directory.")
    parser.add_argument("--sample-dir", type=Path, default=DEFAULT_SAMPLE_DIR, help="Sample watch directory.")
    parser.add_argument("--project", type=Path, default=None, help="Ableton .als project file to watch.")
    parser.add_argument("--export-dir", type=Path, default=None, help="Export directory to watch for WAV/AIFF.")
    parser.add_argument("--manifest-dir", type=Path, default=DEFAULT_MANIFEST_DIR, help="Manifest output directory.")
    parser.add_argument(
        "--session-id",
        default=None,
        help="Optional local capture-session identifier. A unique value is generated by default.",
    )
    parser.add_argument("--stem-id", default="stem-1", help="Identifier for the single routed stem.")
    parser.add_argument(
        "--source-category",
        choices=SOURCE_CATEGORIES,
        default="unknown",
        help="Producer-declared source category. Defaults to unknown.",
    )
    parser.add_argument(
        "--signing-key",
        type=Path,
        default=DEFAULT_SIGNING_KEY,
        help="Local software integrity-key path. This is not hardware attestation.",
    )
    parser.add_argument(
        "--portable-private-key",
        type=Path,
        default=DEFAULT_PRIVATE_KEY,
        help="Ed25519 private key for demo integrity signing.",
    )
    parser.add_argument(
        "--portable-public-key",
        type=Path,
        default=DEFAULT_PUBLIC_KEY,
        help="Portable Ed25519 public key written for independent verification.",
    )
    parser.add_argument(
        "--no-html-report",
        action="store_true",
        help="Generate only the JSON manifest, without the derived HTML fight card.",
    )
    parser.add_argument(
        "--open-artifacts",
        action="store_true",
        help="Open each generated fight card with the macOS default browser.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s [%(threadName)s] %(message)s",
        datefmt="%H:%M:%S",
    )
    args = parse_args(argv)
    daemon = Daemon(
        udp_port=args.port,
        evidence_dir=args.evidence_dir,
        sample_dir=args.sample_dir,
        project_path=args.project,
        export_dir=args.export_dir,
        manifest_dir=args.manifest_dir,
        session_id=args.session_id,
        stem_id=args.stem_id,
        source_category=args.source_category,
        signing_key_path=args.signing_key,
        portable_private_key_path=args.portable_private_key,
        portable_public_key_path=args.portable_public_key,
        generate_html_report=not args.no_html_report,
        open_artifacts=args.open_artifacts,
    )
    signal.signal(signal.SIGTERM, lambda _signum, _frame: daemon.stop())
    daemon.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
