from __future__ import annotations

import argparse
import hashlib
import json
import math
import socket
import struct
import subprocess
import sys
import threading
import time
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from daemon.__main__ import Daemon
from daemon.bundle import verify_evidence_bundle
from daemon.verify import verify_manifest

SAMPLE_RATE = 44_100
WINDOW_SIZE = 4096


def _demo_samples(window_count: int = 40) -> list[float]:
    """Deterministic, varied synthetic source with window and intra-window shape."""
    frequencies = (196.0, 293.66, 440.0, 659.25, 329.63, 246.94, 523.25)
    levels = (0.12, 0.30, 0.18, 0.38, 0.09, 0.26, 0.16, 0.34)
    quarter_shape = (0.55, 1.0, 0.72, 0.88)
    samples: list[float] = []
    for window_index in range(window_count):
        frequency = frequencies[window_index % len(frequencies)]
        level = levels[(window_index * 3) % len(levels)]
        for offset in range(WINDOW_SIZE):
            absolute = window_index * WINDOW_SIZE + offset
            shape = quarter_shape[min(3, offset * 4 // WINDOW_SIZE)]
            fundamental = math.sin(2.0 * math.pi * frequency * absolute / SAMPLE_RATE)
            harmonic = 0.23 * math.sin(2.0 * math.pi * frequency * 2.01 * absolute / SAMPLE_RATE)
            samples.append(level * shape * (fundamental + harmonic))
    return samples


def _write_wav(path: Path, samples: list[float], sample_rate: int = SAMPLE_RATE) -> None:
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(b"".join(
            struct.pack("<h", max(-32768, min(32767, round(value * 32767))))
            for value in samples
        ))


def _window_features(window: list[float]) -> tuple[float, float, float, list[float]]:
    rms = math.sqrt(sum(value * value for value in window) / len(window))
    crossings = sum(
        1 for pos in range(1, len(window))
        if (window[pos] >= 0) != (window[pos - 1] >= 0)
    )
    crest = max(abs(value) for value in window) / rms if rms > 1e-9 else 0.0
    envelope: list[float] = []
    for segment in range(4):
        values = window[segment * len(window) // 4 : (segment + 1) * len(window) // 4]
        segment_rms = math.sqrt(sum(value * value for value in values) / len(values))
        envelope.append(segment_rms / rms if rms > 1e-9 else 0.0)
    return rms, crossings / (len(window) - 1), crest, envelope


def _events(
    samples: list[float],
    instance_id: str,
    plugin_session_id: str,
) -> list[dict[str, object]]:
    events: list[dict[str, object]] = []
    previous = "genesis"
    windows = len(samples) // WINDOW_SIZE
    for index in range(windows):
        window = samples[index * WINDOW_SIZE : (index + 1) * WINDOW_SIZE]
        rms, zcr, crest, envelope = _window_features(window)
        digest = hashlib.sha256(
            previous.encode() + struct.pack(f"<{len(window)}f", *window)
        ).hexdigest()
        sequence = index + 1
        events.append({
            "event_type": "buffer_hash",
            "proof_level": "directly_observed",
            "timestamp_ms": 10_000 + round(index * WINDOW_SIZE / SAMPLE_RATE * 1000),
            "sample_position": (index + 1) * WINDOW_SIZE,
            "plugin_instance_id": instance_id,
            "plugin_capture_session_id": plugin_session_id,
            "event_sequence": sequence,
            "window_hash": digest,
            "prev_hash": previous,
            "rms_level": rms,
            "zero_crossing_rate": zcr,
            "crest_factor": crest,
            "energy_envelope": envelope,
            "spectral_centroid_hz": 0.0,
            "sample_rate_hz": SAMPLE_RATE,
            "channel_count": 1,
            "window_size_samples": WINDOW_SIZE,
            "telemetry": {
                "buffers_submitted": sequence * 8,
                "samples_submitted": sequence * WINDOW_SIZE,
                "windows_hashed": sequence,
                "fifo_samples_dropped": 0,
                "fifo_windows_dropped": 0,
                "midi_events_dropped": 0,
                "events_prepared": sequence,
                "udp_sends_attempted": sequence,
                "udp_sends_failed": 0,
            },
        })
        previous = digest
    return events


def _send_with_acknowledgements(
    events: list[dict[str, object]],
    port: int,
) -> dict[str, object]:
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("127.0.0.1", 0))
    sock.settimeout(1.0)
    acknowledgements: list[dict[str, object]] = []
    try:
        for event in events:
            telemetry = event.get("telemetry")
            if isinstance(telemetry, dict):
                previous = acknowledgements[-1] if acknowledgements else {}
                telemetry["daemon_acknowledgements_processed"] = len(acknowledgements)
                telemetry["daemon_highest_accepted_sequence"] = previous.get(
                    "highest_accepted_sequence", 0
                )
                telemetry["daemon_highest_contiguous_sequence"] = previous.get(
                    "highest_contiguous_sequence", 0
                )
                telemetry["daemon_ack_session_mismatches_ignored"] = 0
                telemetry["daemon_restarts_observed"] = 0
            sock.sendto(
                json.dumps(event, separators=(",", ":")).encode(),
                ("127.0.0.1", port),
            )
            payload, _address = sock.recvfrom(8192)
            acknowledgement = json.loads(payload)
            if acknowledgement.get("plugin_instance_id") != event["plugin_instance_id"]:
                raise RuntimeError("daemon acknowledgement plug-in instance mismatch")
            if acknowledgement.get("plugin_capture_session_id") != event["plugin_capture_session_id"]:
                raise RuntimeError("daemon acknowledgement capture-session mismatch")
            if not acknowledgement.get("accepted"):
                raise RuntimeError(f"synthetic event rejected: {acknowledgement.get('reason')}")
            acknowledgements.append(acknowledgement)
    finally:
        sock.close()
    final = acknowledgements[-1] if acknowledgements else {}
    if final.get("highest_contiguous_sequence") != len(events):
        raise RuntimeError("daemon acknowledgement did not close the synthetic event prefix")
    return {
        "processed": len(acknowledgements),
        "highest_accepted_sequence": final.get("highest_accepted_sequence", 0),
        "highest_contiguous_sequence": final.get("highest_contiguous_sequence", 0),
        "daemon_instance_id": final.get("daemon_instance_id"),
    }


def run(
    output: Path,
    export_only: bool = False,
    open_artifacts: bool = False,
    time_anchor_url: str | None = None,
) -> Path:
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    session = output.expanduser().resolve() / f"synthetic-{stamp}-{time.time_ns() % 1_000_000:06d}"
    evidence = session / "evidence"
    samples_dir = session / "samples"
    exports = session / "exports"
    manifests = session / "manifests"
    for path in (samples_dir, exports, manifests):
        path.mkdir(parents=True, exist_ok=True)

    daemon = Daemon(
        udp_port=0,
        evidence_dir=evidence,
        sample_dir=samples_dir,
        export_dir=exports,
        manifest_dir=manifests,
        session_id=session.name,
        stem_id="synthetic-stem",
        source_category="generator",
        time_anchor_url=time_anchor_url,
    )
    port = daemon.receiver.sock.getsockname()[1]
    thread = threading.Thread(target=daemon.run, daemon=True)
    thread.start()
    acknowledgement_summary: dict[str, object] = {"processed": 0}
    manifest_path: Path | None = None
    try:
        time.sleep(0.2)
        source = _demo_samples()
        if not export_only:
            acknowledgement_summary = _send_with_acknowledgements(
                _events(source, "synthetic-plugin-001", "synthetic-plugin-session-001"),
                port,
            )
            # Three exact routed-window offsets plus a fixed gain change exercise
            # bounded alignment without pretending mastering invariance.
            export_samples = [0.0] * (3 * WINDOW_SIZE) + [sample * 0.58 for sample in source]
            export_path = exports / "presenter_export.wav"
        else:
            export_samples = _demo_samples(window_count=12)
            export_path = exports / "unobserved_export.wav"
        _write_wav(export_path, export_samples)

        deadline = time.monotonic() + 12.0
        while time.monotonic() < deadline:
            candidates = sorted(manifests.glob("*_manifest.json"))
            if candidates:
                manifest_path = candidates[-1]
                break
            time.sleep(0.2)
        if manifest_path is None:
            raise RuntimeError("synthetic rehearsal timed out waiting for a manifest")
    finally:
        daemon.stop()
        thread.join(timeout=3)

    result = verify_manifest(manifest_path, signing_key_path=None)
    data = json.loads(manifest_path.read_text())
    presentation = data.get("presentation", {})
    bundle_path = manifests / str(presentation.get("evidence_bundle", ""))
    bundle_index_path = manifests / str(presentation.get("bundle_index", ""))
    bundle_errors = verify_evidence_bundle(bundle_index_path, bundle_path)
    summary = {
        "session": str(session),
        "dashboard": str(session / "dashboard.html"),
        "manifest": str(manifest_path),
        "fight_card": str(manifests / str(presentation.get("html_report", ""))),
        "evidence_bundle": str(bundle_path),
        "bundle_index": str(bundle_index_path),
        "verifier_outcome": result.outcome,
        "coverage": data["observation_coverage"]["status"],
        "association": data["stem_export_association"]["status"],
        "association_offset_seconds": data["stem_export_association"].get("best_offset_seconds"),
        "acknowledgement": acknowledgement_summary,
        "bundle_integrity": "verified" if not bundle_errors else bundle_errors,
    }
    print(json.dumps(summary, indent=2))
    if result.outcome != "verified":
        raise RuntimeError(f"synthetic verifier outcome was {result.outcome}")
    if bundle_errors:
        raise RuntimeError(f"synthetic evidence bundle failed: {bundle_errors}")
    if not export_only and data["stem_export_association"]["status"] != "inferred_match":
        raise RuntimeError("synthetic transformed export did not establish inferred alignment")
    if open_artifacts:
        subprocess.run(["open", str(session / "dashboard.html")], check=False)
        subprocess.run(
            ["open", str(manifests / str(presentation.get("html_report", "")))],
            check=False,
        )
    return manifest_path


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a synthetic end-to-end provenance rehearsal.")
    parser.add_argument("--output", type=Path, default=Path("demo-output/rehearsals"))
    parser.add_argument("--export-only", action="store_true")
    parser.add_argument("--open", action="store_true", dest="open_artifacts")
    parser.add_argument(
        "--time-anchor",
        nargs="?",
        const="http://timestamp.digicert.com",
        default=None,
        metavar="TSA_URL",
        help="Anchor the export hash at an RFC 3161 TSA during sealing (needs network).",
    )
    args = parser.parse_args()
    run(args.output, args.export_only, args.open_artifacts, args.time_anchor)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
