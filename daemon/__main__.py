from __future__ import annotations

import argparse
import hashlib
import json
import logging
import socket
import threading
import time
import uuid
from pathlib import Path

from daemon.common import append_jsonl, sha256_file
from daemon.correlation_engine.engine import CorrelationEngine, LayerEvent
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
        hardware_provider: HardwareProvider | None = None,
        generate_html_report: bool = True,
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
        self._session_events: list[dict[str, object]] = []
        self._active_layers: set[str] = {"sample_watcher"}
        self._latest_project_snapshot = None
        self._plugin_seen = False
        self._stop = threading.Event()

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
            self._session_events.append(event)

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
        for t in threads:
            t.start()

        try:
            while not self._stop.is_set():
                self._stop.wait(1.0)
        except KeyboardInterrupt:
            log.info("Shutting down")
            self._stop.set()
        finally:
            self._stop.set()
            self.receiver.close()

    def stop(self) -> None:
        self._stop.set()
        self.receiver.close()

    def _run_udp_receiver(self) -> None:
        log.info("UDP receiver on port %d", self.receiver.port)
        self.receiver.sock.settimeout(1.0)

        while not self._stop.is_set():
            try:
                data, _addr = self.receiver.sock.recvfrom(65535)
            except socket.timeout:
                continue
            except OSError:
                if self._stop.is_set():
                    break
                log.exception("UDP socket error")
                continue

            event = self.receiver.process_packet(data)
            if event is None:
                continue

            et = str(event.get("event_type", ""))
            layer = _event_type_to_layer(et)
            with self._session_lock:
                self._session_events.append(event)
                self._active_layers.add(layer)
            if not self._plugin_seen:
                self._plugin_seen = True
                log.info("Capture plugin evidence stream detected")
            layer_event = LayerEvent(
                layer=layer,
                event_type=et,
                timestamp_ms=int(event.get("timestamp_ms", 0)),
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
                self._append_event(event)

                layer_event = LayerEvent(
                    layer="sample_watcher",
                    event_type="sample_file_observed",
                    timestamp_ms=int(time.time() * 1000),
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
                            "timestamp_ms": int(time.time() * 1000),
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
                            timestamp_ms=int(time.time() * 1000),
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
                        self._generate_manifest(path)
                        self._export_seen[resolved] = self._export_signature(path)
                    except Exception:
                        log.exception("Manifest generation failed for %s; will retry", path)
            except OSError:
                log.exception("Export watcher could not scan %s", self.export_dir)

            time.sleep(2.0)

    def _generate_manifest(self, export_path: Path) -> Path:
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
        ))

        first_hash_ms = 0
        last_hash_ms = 0
        first_received_at: str | None = None
        last_received_at: str | None = None
        last_window_hash = ""
        chain_genesis = "genesis"
        chain_length = 0
        stem_sr = 0
        stem_ch = 0

        with self._session_lock:
            events_snapshot = list(self._session_events)

        for event in events_snapshot:
            et = event.get("event_type")
            if et == "buffer_hash":
                chain_length += 1
                ts = int(event.get("timestamp_ms", 0))
                if chain_length == 1:
                    first_hash_ms = ts
                    chain_genesis = str(event.get("prev_hash", "genesis"))
                    first_received_at = str(event.get("received_at", "")) or None
                last_hash_ms = ts
                last_received_at = str(event.get("received_at", "")) or last_received_at
                last_window_hash = str(event.get("window_hash", ""))
                stem_sr = int(event.get("sample_rate_hz", stem_sr))
                stem_ch = int(event.get("channel_count", stem_ch))
            elif et == "sample_file_observed":
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
            ))

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
        for evidence_file in self.evidence_dir.glob("*.jsonl"):
            snapshot_bytes = evidence_file.read_bytes()
            digest = hashlib.sha256(snapshot_bytes).hexdigest()
            evidence_hashes[evidence_file.name] = digest
            evidence_files[evidence_file.name] = {
                "sha256": digest,
                "byte_length": len(snapshot_bytes),
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

        report_path = self.manifest_dir / f"{export_path.stem}_provenance.html"
        manifest["presentation"] = {
            "html_report": report_path.name if self.generate_html_report else None,
            "derived_from": f"{export_path.stem}_manifest.json",
            "apw:proof_level": "directly_observed",
        }

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

        manifest_path = self.manifest_dir / f"{export_path.stem}_manifest.json"
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        with manifest_path.open("w", encoding="utf-8") as f:
            json.dump(manifest, f, indent=2, ensure_ascii=False)
            f.write("\n")
        log.info("Manifest written: %s", manifest_path)
        if self.generate_html_report:
            write_html_report(manifest, report_path)
            log.info("Fight-card report written: %s", report_path)
        if chain_length == 0:
            log.warning("Export was hashed, but no routed-audio hash events were received")
        return manifest_path

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
        "--no-html-report",
        action="store_true",
        help="Generate only the JSON manifest, without the derived HTML fight card.",
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
        generate_html_report=not args.no_html_report,
    )
    daemon.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
