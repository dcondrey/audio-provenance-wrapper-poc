from __future__ import annotations

import argparse
import hashlib
import json
import math
import socket
import struct
import sys
import threading
import time
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from daemon.__main__ import Daemon
from daemon.verify import verify_manifest


def _write_tone(path: Path, duration: float = 3.0, sample_rate: int = 44_100) -> list[float]:
    samples = [
        0.35 * math.sin(2.0 * math.pi * 440.0 * index / sample_rate)
        for index in range(round(duration * sample_rate))
    ]
    with wave.open(str(path), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(sample_rate)
        handle.writeframes(b"".join(
            struct.pack("<h", max(-32768, min(32767, round(value * 32767))))
            for value in samples
        ))
    return samples


def _events(samples: list[float], instance_id: str) -> list[dict[str, object]]:
    events: list[dict[str, object]] = []
    previous = "genesis"
    window_size = 4096
    windows = len(samples) // window_size
    for index in range(windows):
        window = samples[index * window_size : (index + 1) * window_size]
        rms = math.sqrt(sum(value * value for value in window) / len(window))
        crossings = sum(
            1 for pos in range(1, len(window))
            if (window[pos] >= 0) != (window[pos - 1] >= 0)
        )
        digest = hashlib.sha256(
            previous.encode() + struct.pack(f"<{len(window)}f", *window)
        ).hexdigest()
        sequence = index + 1
        events.append({
            "event_type": "buffer_hash",
            "proof_level": "directly_observed",
            "timestamp_ms": 10_000 + index * 93,
            "sample_position": (index + 1) * window_size,
            "plugin_instance_id": instance_id,
            "plugin_capture_session_id": "synthetic-plugin-session",
            "event_sequence": sequence,
            "window_hash": digest,
            "prev_hash": previous,
            "rms_level": rms,
            "zero_crossing_rate": crossings / (len(window) - 1),
            "spectral_centroid_hz": 440.0,
            "sample_rate_hz": 44_100,
            "channel_count": 1,
            "window_size_samples": window_size,
            "telemetry": {
                "buffers_submitted": sequence * 8,
                "samples_submitted": sequence * window_size,
                "windows_hashed": sequence,
                "fifo_samples_dropped": 0,
                "fifo_windows_dropped": 0,
                "events_prepared": sequence,
                "udp_sends_attempted": sequence,
                "udp_sends_failed": 0,
            },
        })
        previous = digest
    return events


def run(output: Path, export_only: bool = False) -> Path:
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
    )
    port = daemon.receiver.sock.getsockname()[1]
    thread = threading.Thread(target=daemon.run, daemon=True)
    thread.start()
    time.sleep(0.25)

    tone_path = exports / ("unobserved_export.wav" if export_only else "aligned_export.wav")
    tone = _write_tone(tone_path)
    if not export_only:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            for event in _events(tone, "synthetic-plugin-001"):
                sock.sendto(json.dumps(event, separators=(",", ":")).encode(), ("127.0.0.1", port))
        finally:
            sock.close()
        time.sleep(0.5)
        _write_tone(tone_path)

    deadline = time.monotonic() + 12.0
    manifest_path: Path | None = None
    while time.monotonic() < deadline:
        candidates = sorted(manifests.glob("*_manifest.json"))
        if candidates:
            manifest_path = candidates[-1]
            break
        time.sleep(0.25)
    daemon.stop()
    thread.join(timeout=3)
    if manifest_path is None:
        raise RuntimeError("synthetic rehearsal timed out waiting for a manifest")
    result = verify_manifest(manifest_path, signing_key_path=None)
    data = json.loads(manifest_path.read_text())
    print(json.dumps({
        "session": str(session),
        "manifest": str(manifest_path),
        "verifier_outcome": result.outcome,
        "coverage": data["observation_coverage"]["status"],
        "association": data["stem_export_association"]["status"],
    }, indent=2))
    if result.outcome != "verified":
        raise RuntimeError(f"synthetic verifier outcome was {result.outcome}")
    return manifest_path


def main() -> int:
    parser = argparse.ArgumentParser(description="Run a synthetic end-to-end provenance rehearsal.")
    parser.add_argument("--output", type=Path, default=Path("demo-output/rehearsals"))
    parser.add_argument("--export-only", action="store_true")
    args = parser.parse_args()
    run(args.output, args.export_only)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
