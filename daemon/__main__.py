from __future__ import annotations

import argparse
import logging
import signal
import socket
import threading
import time
import uuid
from collections import OrderedDict, deque
from pathlib import Path

from daemon import status as _status
from daemon.common import append_jsonl, utc_timestamp
from daemon.correlation_engine.engine import CorrelationEngine, LayerEvent
from daemon.evidence_receiver.receiver import EvidenceReceiver
from daemon.hardware_attestation.provider import HardwareProvider, SoftwareProvider, detect_provider
from daemon.manifest_builder import generator as _manifest_generator
from daemon.sample_watcher.watcher import SampleWatcher
from daemon.signing import DEFAULT_PRIVATE_KEY, DEFAULT_PUBLIC_KEY, Ed25519Signer
from daemon.time_anchor.anchor import DEFAULT_TSA_URL, RFC3161Provider, TimeAnchorService

log = logging.getLogger(__name__)

# Same bound rationale as receiver.MAX_TRACKED_STREAMS: both tables are keyed
# by wire-controlled values and rendered into the signed manifest.
MAX_TRACKED_PLUGIN_KEYS = 64

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
        time_anchor_url: str | None = None,
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
        self._time_anchor = (
            TimeAnchorService(RFC3161Provider(time_anchor_url)) if time_anchor_url else None
        )

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
        # Bounded like receiver._stream_states: both are keyed by wire-controlled
        # values and exported into the signed manifest, so unbounded growth is a
        # local-DoS and manifest-spam vector. OrderedDict keys act as an LRU set.
        self._plugin_instance_ids: OrderedDict[str, None] = OrderedDict()
        self._latest_plugin_telemetry: OrderedDict[str, int] = OrderedDict()
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
            self._plugin_instance_ids[instance_id] = None
            self._plugin_instance_ids.move_to_end(instance_id)
            while len(self._plugin_instance_ids) > MAX_TRACKED_PLUGIN_KEYS:
                evicted, _ = self._plugin_instance_ids.popitem(last=False)
                log.warning("Plugin instance table full; evicted least-recent id %s", evicted)
            telemetry = event.get("telemetry")
            if isinstance(telemetry, dict):
                for key, value in telemetry.items():
                    if isinstance(value, int):
                        self._latest_plugin_telemetry[str(key)] = value
                        self._latest_plugin_telemetry.move_to_end(str(key))
                while len(self._latest_plugin_telemetry) > MAX_TRACKED_PLUGIN_KEYS:
                    self._latest_plugin_telemetry.popitem(last=False)
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
                # Worker loops all degrade on exceptions; the main loop must not
                # be the one thread a bad status write can kill.
                try:
                    self._write_status("active" if recently_active else "idle")
                except Exception:
                    log.exception("Status write failed; daemon continues")
        except KeyboardInterrupt:
            log.info("Shutting down")
            self._stop.set()
        finally:
            self._stop.set()
            self.receiver.close()
            # IMPORTANT: workers write evidence/status/manifests; join them before
            # the session_end record so nothing lands after status says "stopped".
            for t in threads:
                t.join(timeout=10.0)
                if t.is_alive():
                    log.warning("Worker thread %s did not stop within 10s", t.name)
            self._write_evidence("session_events.jsonl", {
                "event_type": "session_end",
                "capture_session_id": self.session_id,
                "ended_at": utc_timestamp(),
                "daemon_monotonic_ms": int(time.monotonic_ns() // 1_000_000),
                "proof_level": "directly_observed",
                "diagnostics": self.receiver.diagnostics(),
            })
            try:
                self._write_status("stopped")
            except Exception:
                log.exception("Final status write failed")

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

            try:
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
            except Exception:
                # One bad datagram must degrade, not kill the receiver thread,
                # matching the sample/project/export worker-loop pattern.
                log.exception("UDP packet processing error")

    def _run_sample_watcher(self) -> None:
        self.sample_watcher.mark_existing_seen()
        log.info("Sample watcher on %s", self.sample_watcher.watch_dir)

        while not self._stop.is_set():
            try:
                events = self.sample_watcher.scan_once()
            except Exception:
                log.exception("Sample watcher error")
                self._stop.wait(self.sample_watcher.poll_interval_seconds)
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

            self._stop.wait(self.sample_watcher.poll_interval_seconds)

    def _run_project_watcher(self) -> None:
        from daemon.project_differ.differ import extract_snapshot, compute_diff

        log.info("Project watcher on %s", self.project_path)
        prev_snapshot = None
        prev_mtime_ns = 0

        while not self._stop.is_set():
            try:
                stat = self.project_path.stat()
            except OSError:
                self._stop.wait(2.0)
                continue

            if stat.st_mtime_ns != prev_mtime_ns:
                prev_mtime_ns = stat.st_mtime_ns
                try:
                    snapshot = extract_snapshot(self.project_path)
                except Exception:
                    log.exception("Failed to parse %s", self.project_path)
                    self._stop.wait(2.0)
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

            self._stop.wait(2.0)

    def _run_export_watcher(self) -> None:
        log.info("Export watcher on %s", self.export_dir)
        AUDIO_EXTENSIONS = {".wav", ".aiff", ".aif"}

        try:
            for existing in self.export_dir.iterdir():
                if existing.is_file() and existing.suffix.lower() in AUDIO_EXTENSIONS:
                    try:
                        self._export_seen[str(existing.resolve())] = self._export_signature(existing)
                    except OSError:
                        continue
        except OSError:
            log.exception("Export watcher could not scan %s at startup", self.export_dir)

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

            self._stop.wait(2.0)

    def _generate_manifest(self, export_path: Path, export_version: int = 1) -> Path:
        return _manifest_generator.generate_manifest(self, export_path, export_version)

    def _derive_coverage(self, chain_length: int) -> dict[str, object]:
        return _manifest_generator.derive_coverage(self, chain_length)

    def _derive_forgery_analysis(self, events_snapshot: list[dict[str, object]]) -> dict[str, object]:
        return _manifest_generator.derive_forgery_analysis(events_snapshot)

    def _derive_readiness(self) -> dict[str, object]:
        return _status.derive_readiness(self)

    def _session_diagnostics(self) -> dict[str, object]:
        return _status.session_diagnostics(self)

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
        return _status.build_handoff(
            self,
            export_hash=export_hash,
            association=association,
            coverage=coverage,
            evidence_files=evidence_files,
            manifest_name=manifest_name,
            bundle_name=bundle_name,
            bundle_index_name=bundle_index_name,
        )

    def _status_link(self, path: Path | None) -> str | None:
        return _status.status_link(path, self._status_path.parent)

    def _write_status(self, state: str) -> None:
        _status.write_status(self, state)

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
        "--time-anchor",
        nargs="?",
        const=DEFAULT_TSA_URL,
        default=None,
        metavar="TSA_URL",
        help=(
            "Anchor each export hash at an RFC 3161 TSA (default server if no URL given). "
            "Requires network access at manifest-sealing time; degrades to an explicit "
            "unavailable record on failure."
        ),
    )
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
        time_anchor_url=args.time_anchor,
    )
    signal.signal(signal.SIGTERM, lambda _signum, _frame: daemon.stop())
    daemon.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
