from __future__ import annotations

import argparse
import json
import logging
import socket
import time
import uuid
from pathlib import Path

from daemon.common import append_jsonl, utc_timestamp
from .taxonomy import validate_event

log = logging.getLogger(__name__)


class EvidenceReceiver:

    def __init__(
        self,
        host: str = "127.0.0.1",
        port: int = 9876,
        evidence_path: Path = Path("evidence/plugin_events.jsonl"),
        capture_session_id: str | None = None,
        stem_id: str | None = None,
    ) -> None:
        self.host = host
        self.port = port
        self.evidence_path = evidence_path.expanduser()
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind((host, port))
        self.capture_session_id = capture_session_id
        self.stem_id = stem_id
        self.event_count = 0
        self.packet_count = 0
        self.rejected_count = 0
        self.sequence_gap_count = 0
        self.sequence_missing_count = 0
        self.sequence_out_of_order_count = 0
        self.hash_chain_break_count = 0
        self._last_sequence_by_instance: dict[str, int] = {}
        self._last_window_hash_by_instance: dict[str, str] = {}
        self._receiver_instance_id = f"receiver-{uuid.uuid4().hex[:12]}"

    def process_packet(self, data: bytes) -> dict[str, object] | None:
        self.packet_count += 1
        try:
            event = json.loads(data.decode("utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError):
            self._reject("invalid JSON/UTF-8")
            return None

        valid, error = validate_event(event)
        if not valid:
            self._reject(error)
            return None

        received_at_ms = int(time.time() * 1000)
        received_monotonic_ms = int(time.monotonic_ns() // 1_000_000)
        source_timestamp_ms = event.get("timestamp_ms")
        event["source_timestamp_ms"] = source_timestamp_ms
        event["received_at"] = utc_timestamp(received_at_ms / 1000)
        event["received_at_ms"] = received_at_ms
        event["daemon_received_monotonic_ms"] = received_monotonic_ms
        if self.capture_session_id is not None:
            event["capture_session_id"] = self.capture_session_id
        if self.stem_id is not None:
            event["stem_id"] = self.stem_id
        instance_id = str(event.get("plugin_instance_id") or "unknown_plugin_instance")
        event["plugin_instance_id"] = instance_id

        sequence = event.get("event_sequence")
        if isinstance(sequence, int) and sequence > 0:
            previous = self._last_sequence_by_instance.get(instance_id)
            if previous is not None:
                if sequence <= previous:
                    self.sequence_out_of_order_count += 1
                    self._reject(
                        f"duplicate/out-of-order sequence for {instance_id}: {sequence} <= {previous}"
                    )
                    return None
                if sequence > previous + 1:
                    gap = sequence - previous - 1
                    self.sequence_gap_count += gap
                    log.warning(
                        "UDP sequence gap: instance=%s previous=%d current=%d missing=%d",
                        instance_id,
                        previous,
                        sequence,
                        gap,
                    )
            self._last_sequence_by_instance[instance_id] = sequence
        else:
            self.sequence_missing_count += 1

        if event.get("event_type") == "buffer_hash":
            expected = self._last_window_hash_by_instance.get(instance_id)
            previous_hash = str(event.get("prev_hash", ""))
            if expected is not None and previous_hash != expected:
                self.hash_chain_break_count += 1
                log.warning(
                    "Hash-chain break: instance=%s expected=%s received=%s",
                    instance_id,
                    expected[:12],
                    previous_hash[:12],
                )
            self._last_window_hash_by_instance[instance_id] = str(event.get("window_hash", ""))

        event["daemon_event_id"] = (
            f"{self.capture_session_id or self._receiver_instance_id}:{self.event_count + 1}"
        )
        self._write_event(event)
        self.event_count += 1
        return event

    def _reject(self, reason: str) -> None:
        self.rejected_count += 1
        if self.rejected_count <= 5 or self.rejected_count % 100 == 0:
            log.warning("Rejected UDP event #%d: %s", self.rejected_count, reason)

    def diagnostics(self) -> dict[str, int]:
        return {
            "packets_received": self.packet_count,
            "events_received": self.event_count,
            "events_rejected": self.rejected_count,
            "sequence_gaps": self.sequence_gap_count,
            "events_missing_sequence": self.sequence_missing_count,
            "sequence_out_of_order": self.sequence_out_of_order_count,
            "hash_chain_breaks": self.hash_chain_break_count,
        }

    def _write_event(self, event: dict[str, object]) -> None:
        append_jsonl(self.evidence_path, event)

    def close(self) -> None:
        self.sock.close()

    def run_forever(self) -> None:
        self.evidence_path.parent.mkdir(parents=True, exist_ok=True)
        log.info("Listening on %s:%d; writing %s", self.host, self.port, self.evidence_path)
        while True:
            data, _addr = self.sock.recvfrom(4096)
            event = self.process_packet(data)
            if event is not None:
                log.debug("Received: %s", event.get("event_type"))


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Receive plugin observation events via UDP and write evidence JSONL.",
    )
    parser.add_argument("--host", default="127.0.0.1", help="Host to bind to.")
    parser.add_argument("--port", type=int, default=9876, help="UDP port to listen on.")
    parser.add_argument(
        "--evidence-file",
        type=Path,
        default=Path("evidence/plugin_events.jsonl"),
        help="JSONL output path.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    args = parse_args(argv)
    receiver = EvidenceReceiver(args.host, args.port, args.evidence_file)
    try:
        receiver.run_forever()
    except KeyboardInterrupt:
        return 0
    finally:
        receiver.close()
    return 0
