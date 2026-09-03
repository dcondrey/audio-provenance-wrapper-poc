"""Regenerate coverage_oracle.json and manifest_key_order.json from the Python
daemon, which is the behavioural specification for the Rust port.

    cd <repo root> && /usr/bin/python3 rust/apw-daemon/tests/fixtures/generate_coverage_fixtures.py

Reads daemon/manifest_builder/generator.py::derive_coverage directly; no expected
byte in the fixture is written by hand.
"""

from __future__ import annotations

import json
import pathlib
import sys
import threading
import types

REPO_ROOT = pathlib.Path(__file__).resolve().parents[4]
sys.path.insert(0, str(REPO_ROOT))

from daemon.manifest_builder.generator import derive_coverage  # noqa: E402

CLEAN_DIAGNOSTICS = {
    "packets_received": 3, "events_received": 3, "events_rejected": 0,
    "sequence_gaps": 0, "events_missing_sequence": 0, "sequence_out_of_order": 0,
    "hash_chain_breaks": 0, "stream_evictions": 0,
    "daemon_acknowledgements_attempted": 3, "daemon_acknowledgements_sent": 3,
    "daemon_acknowledgements_failed": 0,
}
FULL_TELEMETRY = {
    "buffers_submitted": 10, "samples_submitted": 100, "windows_hashed": 3,
    "fifo_samples_dropped": 0, "fifo_windows_dropped": 0, "midi_events_dropped": 0,
    "events_prepared": 3, "udp_sends_attempted": 3, "udp_sends_failed": 0,
}


def coverage(diagnostics, telemetry, instances, chain_length, feature_drops):
    receiver = types.SimpleNamespace(diagnostics=lambda: dict(diagnostics))
    daemon = types.SimpleNamespace(
        receiver=receiver,
        _session_lock=threading.Lock(),
        _latest_plugin_telemetry=dict(telemetry),
        _plugin_instance_ids={f"instance-{n}": None for n in range(instances)},
        _feature_window_drops=feature_drops,
    )
    return derive_coverage(daemon, chain_length)


# A manifest the Python pipeline actually produced, not a constructed one: the
# top-level key order is the assembly sequence, and the sequence is the security
# property the Rust port has to reproduce.
REFERENCE_MANIFEST = (
    "demo-output/founder-package/evidence-package-20260831T091551Z/"
    "presenter_export_manifest.json"
)


def write_manifest_key_order() -> int:
    source = REPO_ROOT / REFERENCE_MANIFEST
    document = json.loads(source.read_text(encoding="utf-8"))
    out = pathlib.Path(__file__).with_name("manifest_key_order.json")
    out.write_text(
        json.dumps(
            {"source": REFERENCE_MANIFEST, "keys": list(document.keys())},
            indent=2,
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {out} ({len(document)} keys)")
    return 0


def main() -> int:
    evicted = dict(CLEAN_DIAGNOSTICS, stream_evictions=1)
    partial_telemetry = {k: v for k, v in FULL_TELEMETRY.items() if k != "midi_events_dropped"}
    cases = {
        "complete": coverage(CLEAN_DIAGNOSTICS, FULL_TELEMETRY, 1, 3, 0),
        "no_chain": coverage(CLEAN_DIAGNOSTICS, FULL_TELEMETRY, 1, 0, 0),
        "missing_counter": coverage(CLEAN_DIAGNOSTICS, partial_telemetry, 1, 3, 2),
        "stream_evicted": coverage(evicted, FULL_TELEMETRY, 1, 3, 0),
        "two_plugin_instances": coverage(CLEAN_DIAGNOSTICS, FULL_TELEMETRY, 2, 3, 0),
    }
    rendered = {
        name: json.dumps(record, separators=(",", ":"), ensure_ascii=False)
        for name, record in cases.items()
    }
    out = pathlib.Path(__file__).with_name("coverage_oracle.json")
    out.write_text(json.dumps(rendered, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {out} ({len(rendered)} cases)")
    return write_manifest_key_order()


if __name__ == "__main__":
    raise SystemExit(main())
