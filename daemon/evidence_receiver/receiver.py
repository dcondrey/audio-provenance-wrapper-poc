from __future__ import annotations

import argparse
import json
import logging
import socket
import time
import uuid
from dataclasses import dataclass
from pathlib import Path

from daemon.common import append_jsonl, utc_timestamp
from .taxonomy import validate_event

log = logging.getLogger(__name__)


@dataclass
class _StreamReceiptState:
    highest_accepted_sequence: int = 0
    highest_contiguous_sequence: int = 0
    gaps: int = 0
    rejections: int = 0
    chain_breaks: int = 0


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
        self.acknowledgements_attempted = 0
        self.acknowledgements_sent = 0
        self.acknowledgements_failed = 0
        self._stream_states: dict[tuple[str, str], _StreamReceiptState] = {}
        self._last_window_hash_by_stream: dict[tuple[str, str], str] = {}
        self._receiver_instance_id = f"receiver-{uuid.uuid4().hex[:12]}"

    def process_packet(self, data: bytes) -> dict[str, object] | None:
        event, _ack = self.process_packet_with_ack(data)
        return event

    def process_packet_with_ack(
        self,
        data: bytes,
    ) -> tuple[dict[str, object] | None, dict[str, object]]:
        self.packet_count += 1
        raw_event: dict[str, object] = {}
        try:
            decoded = json.loads(data.decode("utf-8"))
            if isinstance(decoded, dict):
                raw_event = decoded
            else:
                raise ValueError("event must be a JSON object")
        except (json.JSONDecodeError, UnicodeDecodeError, ValueError):
            reason = "invalid JSON/UTF-8 object"
            self._reject(reason)
            return None, self._build_ack(raw_event, False, "rejected_invalid", reason)

        event = raw_event
        instance_id = str(event.get("plugin_instance_id") or "unknown_plugin_instance")
        plugin_session_id = str(
            event.get("plugin_capture_session_id") or "unknown_plugin_capture_session"
        )
        stream_key = (instance_id, plugin_session_id)
        stream = self._stream_states.setdefault(stream_key, _StreamReceiptState())

        valid, error = validate_event(event)
        if not valid:
            self._reject(error)
            stream.rejections += 1
            return None, self._build_ack(event, False, "rejected_invalid", error, stream)

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
        event["plugin_instance_id"] = instance_id
        event["plugin_capture_session_id"] = plugin_session_id

        sequence = event.get("event_sequence")
        receipt_state = "accepted"
        if isinstance(sequence, int) and sequence > 0:
            previous = stream.highest_accepted_sequence
            if previous > 0 and sequence <= previous:
                self.sequence_out_of_order_count += 1
                reason = (
                    f"duplicate/out-of-order sequence for {instance_id}/{plugin_session_id}: "
                    f"{sequence} <= {previous}"
                )
                self._reject(reason)
                stream.rejections += 1
                return None, self._build_ack(
                    event, False, "rejected_duplicate_or_out_of_order", reason, stream
                )
            expected = previous + 1 if previous > 0 else 1
            if sequence > expected:
                gap = sequence - expected
                self.sequence_gap_count += gap
                stream.gaps += gap
                receipt_state = "accepted_with_gap"
                log.warning(
                    "UDP sequence gap: instance=%s plugin_session=%s previous=%d current=%d missing=%d",
                    instance_id,
                    plugin_session_id,
                    previous,
                    sequence,
                    gap,
                )
            stream.highest_accepted_sequence = sequence
            if sequence == stream.highest_contiguous_sequence + 1:
                stream.highest_contiguous_sequence = sequence
        else:
            self.sequence_missing_count += 1
            receipt_state = "accepted_sequence_unknown"

        if event.get("event_type") == "buffer_hash":
            expected = self._last_window_hash_by_stream.get(stream_key)
            previous_hash = str(event.get("prev_hash", ""))
            if expected is not None and previous_hash != expected:
                self.hash_chain_break_count += 1
                stream.chain_breaks += 1
                receipt_state = "accepted_chain_break"
                log.warning(
                    "Hash-chain break: instance=%s plugin_session=%s expected=%s received=%s",
                    instance_id,
                    plugin_session_id,
                    expected[:12],
                    previous_hash[:12],
                )
            self._last_window_hash_by_stream[stream_key] = str(event.get("window_hash", ""))

        event["daemon_event_id"] = (
            f"{self.capture_session_id or self._receiver_instance_id}:{self.event_count + 1}"
        )
        self._write_event(event)
        self.event_count += 1
        return event, self._build_ack(event, True, receipt_state, None, stream)

    def _build_ack(
        self,
        event: dict[str, object],
        accepted: bool,
        receipt_state: str,
        reason: str | None,
        stream: _StreamReceiptState | None = None,
    ) -> dict[str, object]:
        instance_id = str(event.get("plugin_instance_id") or "unknown_plugin_instance")
        plugin_session_id = str(
            event.get("plugin_capture_session_id") or "unknown_plugin_capture_session"
        )
        if stream is None:
            stream = self._stream_states.get((instance_id, plugin_session_id))
        ack: dict[str, object] = {
            "message_type": "daemon_receipt_acknowledgement",
            "protocol": "apw-local-udp-ack-v1",
            "daemon_instance_id": self._receiver_instance_id,
            "daemon_capture_session_id": self.capture_session_id,
            "plugin_instance_id": instance_id,
            "plugin_capture_session_id": plugin_session_id,
            "event_sequence": event.get("event_sequence"),
            "accepted": accepted,
            "receipt_state": receipt_state,
            "highest_accepted_sequence": stream.highest_accepted_sequence if stream else 0,
            "highest_contiguous_sequence": stream.highest_contiguous_sequence if stream else 0,
            "stream_gaps": stream.gaps if stream else 0,
            "stream_rejections": stream.rejections if stream else 0,
            "stream_chain_breaks": stream.chain_breaks if stream else 0,
            "daemon_received_at": utc_timestamp(),
            "operational_scope": (
                "Local daemon validation and persistence receipt only; not identity proof, "
                "remote attestation, DAW trust, or registry confirmation."
            ),
        }
        if reason:
            ack["reason"] = reason
        return ack

    def send_acknowledgement(
        self,
        address: tuple[str, int],
        acknowledgement: dict[str, object],
    ) -> bool:
        self.acknowledgements_attempted += 1
        payload = json.dumps(
            acknowledgement,
            sort_keys=True,
            separators=(",", ":"),
            ensure_ascii=False,
        ).encode("utf-8")
        try:
            sent = self.sock.sendto(payload, address)
        except OSError:
            self.acknowledgements_failed += 1
            return False
        if sent != len(payload):
            self.acknowledgements_failed += 1
            return False
        self.acknowledgements_sent += 1
        return True

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
            "daemon_acknowledgements_attempted": self.acknowledgements_attempted,
            "daemon_acknowledgements_sent": self.acknowledgements_sent,
            "daemon_acknowledgements_failed": self.acknowledgements_failed,
        }

    def receipt_summary(self) -> dict[str, object]:
        streams = [
            {
                "plugin_instance_id": instance_id,
                "plugin_capture_session_id": plugin_session_id,
                "highest_accepted_sequence": state.highest_accepted_sequence,
                "highest_contiguous_sequence": state.highest_contiguous_sequence,
                "gaps": state.gaps,
                "rejections": state.rejections,
                "chain_breaks": state.chain_breaks,
            }
            for (instance_id, plugin_session_id), state in sorted(self._stream_states.items())
        ]
        if not streams:
            status = "unknown"
        elif self.acknowledgements_failed or self.acknowledgements_sent < self.packet_count or any(
            stream["gaps"] or stream["rejections"] or stream["chain_breaks"]
            for stream in streams
        ):
            status = "degraded"
        else:
            status = "issued"
        return {
            "protocol": "apw-local-udp-ack-v1",
            "status": status,
            "daemon_instance_id": self._receiver_instance_id,
            "daemon_capture_session_id": self.capture_session_id,
            "streams": streams,
            "counters": {
                "attempted": self.acknowledgements_attempted,
                "sent": self.acknowledgements_sent,
                "failed": self.acknowledgements_failed,
            },
            "scope": (
                "The daemon directly observed validation, persistence, and local acknowledgement "
                "dispatch. It cannot observe whether every UDP acknowledgement reached the plug-in UI."
            ),
            "apw:proof_level": "directly_observed" if streams else "unknown_unobserved",
        }

    def _write_event(self, event: dict[str, object]) -> None:
        append_jsonl(self.evidence_path, event)

    def close(self) -> None:
        self.sock.close()

    def run_forever(self) -> None:
        self.evidence_path.parent.mkdir(parents=True, exist_ok=True)
        log.info("Listening on %s:%d; writing %s", self.host, self.port, self.evidence_path)
        while True:
            data, address = self.sock.recvfrom(65535)
            event, acknowledgement = self.process_packet_with_ack(data)
            self.send_acknowledgement(address, acknowledgement)
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
