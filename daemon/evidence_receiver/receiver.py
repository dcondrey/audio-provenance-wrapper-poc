from __future__ import annotations

import argparse
import json
import logging
import socket
import time
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

    def process_packet(self, data: bytes) -> dict[str, object] | None:
        try:
            event = json.loads(data.decode("utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError):
            return None

        valid, error = validate_event(event)
        if not valid:
            log.warning("Invalid event: %s", error)
            return None

        received_at_ms = int(time.time() * 1000)
        event["received_at"] = utc_timestamp(received_at_ms / 1000)
        event["received_at_ms"] = received_at_ms
        if self.capture_session_id is not None:
            event["capture_session_id"] = self.capture_session_id
        if self.stem_id is not None:
            event["stem_id"] = self.stem_id
        self._write_event(event)
        self.event_count += 1
        return event

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
