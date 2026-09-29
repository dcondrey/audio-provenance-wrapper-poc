"""Regenerates the apw-core builder / schema / signature fixtures.

Every expected value here is produced by the Python implementation in daemon/,
which is the behavioural specification for the Rust port.

    /Volumes/A/audio-provenance/.venv/bin/python \
        rust/apw-core/tests/generate_parity_fixtures.py
"""

from __future__ import annotations

import json
import pathlib
import sys
import tempfile

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT))

from daemon.common import canonical_json_bytes  # noqa: E402
from daemon.manifest_builder.builder import (  # noqa: E402
    ExportEvidence,
    IngredientEvidence,
    ManifestBuilder,
    StemEvidence,
)
from daemon.schema import validate_manifest_invariants  # noqa: E402
from daemon.signing import Ed25519Signer  # noqa: E402

FIXTURES = pathlib.Path(__file__).resolve().parent / "fixtures"
CREATED_AT = "2025-08-31T00:26:40.500000Z"


def pretty(value: object) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def builder_cases() -> list[dict[str, object]]:
    empty = ManifestBuilder(session_id="", created_at=CREATED_AT)

    full = ManifestBuilder(session_id="sess-café-01", created_at=CREATED_AT)
    full.add_stem(
        StemEvidence(
            stem_id="stem-1",
            hash_chain_root="a" * 64,
            hash_chain_length=12,
            first_observed_ms=1000,
            last_observed_ms=5000,
            sample_rate_hz=44100,
            channel_count=2,
            source_category="synthesised",
            proof_level="directly_observed",
            source_category_proof_level="user_declared",
            hash_chain_genesis="genesis",
            first_received_at=CREATED_AT,
            last_received_at=None,
            plugin_instance_ids=("plug-1", "plug-2"),
        )
    )
    full.add_stem(
        StemEvidence(
            stem_id="stem-é-2",
            hash_chain_root="b" * 64,
            hash_chain_length=7,
            first_observed_ms=-1,
            last_observed_ms=0,
            sample_rate_hz=48000,
            channel_count=1,
            source_category="recorded",
            proof_level="inferred",
        )
    )
    full.set_export(
        ExportEvidence(
            file_path="/tmp/Bounce – final.wav",
            file_name="Bounce – final.wav",
            sha256="c" * 64,
            format="wav",
            file_size_bytes=1234567,
            duration_seconds=12.345678901234567,
            exported_at="2025-08-31T00:26:40Z",
            sample_rate_hz=44100.0,
            channel_count=2,
            export_version=3,
        )
    )
    full.add_ingredient(
        IngredientEvidence(
            file_name="kick.wav",
            sha256="d" * 64,
            proof_level="inferred",
            correlation_confidence=0.9375,
            audio_fingerprint={"rms": 0.0001220703125, "zcr": None},
        )
    )
    full.add_composite_edit({"edit_type": "clip_paste", "timestamp_ms": 42, "confidence": 0.5})
    full.add_composite_edit({"edit_type": "mystery", "timestamp_ms": None, "confidence": None})
    full.set_hardware_binding({"provider": "software", "apw:proof_level": "inferred"})
    full.set_forgery_report({"suspicion_score": 0.0, "flags": []})
    full.coverage = {
        "status": "complete_observed_path",
        "basis": "All hashed windows were received.",
        "counters": {"windows_hashed": 19, "buffer_hash_events_received": 19},
        "apw:proof_level": "directly_observed",
    }
    full.audio_association = {
        "status": "inferred_match",
        "method": "gain_normalised_offset_search",
        "confidence": 0.8125,
        "matched_coverage": 0.75,
        "apw:proof_level": "inferred",
    }
    full.session_diagnostics = {"udp_sends_failed": 0}
    full.set_host_environment(
        {
            "status": "observed",
            "host_recognised": True,
            "host_name": "Ableton Live",
            "host_executable_name": "Live",
            "wrapper_format": "VST3",
            "basis": "The plug-in wrapper named the host application that loaded it.",
            "scope": "host scope",
            "apw:proof_level": "directly_observed",
        }
    )
    full.set_c2pa_claim(
        {
            "status": "embedded",
            "validation": {"state": "verified", "trust_anchor_scope": "self_issued_local_root_only"},
            "hard_binding": {"algorithm": "sha256"},
            "source_sha256_matches_export": True,
            "signer": {"signer_identity": "not_established", "apw:proof_level": "directly_observed"},
            "ingredients": [],
            "apw:proof_level": "directly_observed",
        }
    )

    no_claim_no_association = ManifestBuilder(session_id="s2", created_at=CREATED_AT)
    no_claim_no_association.add_stem(
        StemEvidence(
            stem_id="stem-only",
            hash_chain_root="e" * 64,
            hash_chain_length=3,
            first_observed_ms=0,
            last_observed_ms=1,
            sample_rate_hz=44100,
            channel_count=2,
            source_category="unknown",
            proof_level="directly_observed",
        )
    )

    return [
        {"name": name, "manifest": builder.build(), "pretty": pretty(builder.build())}
        for name, builder in (
            ("empty", empty),
            ("full", full),
            ("stem_only", no_claim_no_association),
        )
    ]


def deep(levels: int) -> dict[str, object]:
    node: object = {"apw:proof_level": "directly_observed"}
    for _ in range(levels):
        node = {"a": node}
    return {"deep": node}


def schema_cases() -> list[dict[str, object]]:
    valid = ManifestBuilder(session_id="s", created_at=CREATED_AT).build()

    missing_everything: dict[str, object] = {}

    bad_proof = json.loads(json.dumps(valid))
    bad_proof["capture_session"]["apw:proof_level"] = "totally_verified"
    bad_proof["observed_stems"] = [{"stem_id": "x"}]

    counters_numeric = json.loads(json.dumps(valid))
    counters_numeric["observation_coverage"] = {
        "status": "complete_observed_path",
        "apw:proof_level": "directly_observed",
        "counters": {
            "windows_hashed": 5,
            "buffer_hash_events_received": 5.0,
            "fifo_samples_dropped": 0.0,
            "fifo_windows_dropped": False,
            "udp_sends_failed": 0,
            "sequence_gaps": 0,
            "hash_chain_breaks": 0,
            "events_prepared": 9,
            "events_received": 9.0,
            "daemon_acknowledgements_sent": 9,
            "daemon_acknowledgements_failed": 0,
        },
    }

    counters_bad = json.loads(json.dumps(counters_numeric))
    counters_bad["observation_coverage"]["counters"]["windows_hashed"] = 4
    counters_bad["observation_coverage"]["counters"]["udp_sends_failed"] = 1
    del counters_bad["observation_coverage"]["counters"]["hash_chain_breaks"]

    association_bad = json.loads(json.dumps(valid))
    association_bad["stem_export_association"] = {
        "status": "inferred_match",
        "apw:proof_level": "directly_observed",
    }

    claim_bad = json.loads(json.dumps(valid))
    claim_bad["c2pa_claim"] = {
        "status": "embedded",
        "apw:proof_level": "inferred",
        "validation": {"state": "totally_trusted", "trust_anchor_scope": "public_ca"},
        "signer": {"signer_identity": "acme", "apw:proof_level": "externally_verified"},
        "ingredients": [{"apw:proof_level": "inferred"}, 7],
        "source_sha256_matches_export": "yes",
    }

    claim_unavailable_bad = json.loads(json.dumps(valid))
    claim_unavailable_bad["c2pa_claim"] = {
        "status": "unavailable",
        "apw:proof_level": "directly_observed",
        "reason": "",
    }

    portable_bad = json.loads(json.dumps(valid))
    portable_bad["portable_signature"] = {
        "signer_identity_proof_level": "externally_verified",
        "trust_scope": "somebody_elses_root",
    }
    portable_bad["export"] = {"sha256": "abc", "apw:proof_level": "directly_observed"}

    host_bad = json.loads(json.dumps(valid))
    host_bad["host_environment"] = {
        "status": "host_unrecognised",
        "host_recognised": True,
        "host_name": "Unknown",
        "apw:proof_level": "directly_observed",
    }

    host_status_bad = json.loads(json.dumps(valid))
    host_status_bad["host_environment"] = {
        "status": "definitely_ableton",
        "apw:proof_level": "directly_observed",
    }

    host_unnamed_bad = json.loads(json.dumps(valid))
    host_unnamed_bad["host_environment"] = {
        "status": "observed",
        "host_recognised": True,
        "host_name": "",
        "apw:proof_level": "inferred",
    }

    inferred_ok = json.loads(json.dumps(valid))
    inferred_ok["host_environment"] = {
        "status": "host_inferred", "host_recognised": False, "host_name": "LMMS",
        "host_executable_name": "lmms", "wrapper_format": "VST3",
        "identification": "inferred_from_executable_name", "host_id": "lmms",
        "source_url": "https://example.org/lmms", "apw:proof_level": "inferred",
    }
    inferred_overclaim = json.loads(json.dumps(inferred_ok))
    inferred_overclaim["host_environment"]["apw:proof_level"] = "directly_observed"
    inferred_overclaim["host_environment"]["host_recognised"] = True
    inferred_overclaim["host_environment"]["host_name"] = ""
    inferred_overclaim["host_environment"]["identification"] = "juce_plugin_host_type"
    inferred_overclaim["host_environment"]["host_id"] = ""
    inferred_overclaim["host_environment"].pop("source_url")

    not_an_object = ["nope"]

    return [
        {"name": name, "manifest": value, "errors": validate_manifest_invariants(value)}
        for name, value in (
            ("valid_builder_output", valid),
            ("missing_everything", missing_everything),
            ("bad_proof_levels", bad_proof),
            ("counters_numeric_equivalence", counters_numeric),
            ("counters_violations", counters_bad),
            ("association_proof_mismatch", association_bad),
            ("c2pa_claim_violations", claim_bad),
            ("c2pa_claim_unavailable_violations", claim_unavailable_bad),
            ("portable_signature_violations", portable_bad),
            ("host_environment_unidentified_violations", host_bad),
            ("host_environment_status_invalid", host_status_bad),
            ("host_environment_unnamed_violations", host_unnamed_bad),
            ("host_environment_inferred_valid", inferred_ok),
            ("host_environment_inferred_violations", inferred_overclaim),
            ("nesting_overflow", deep(70)),
            ("not_an_object", not_an_object),
        )
    ]


def signature_case() -> dict[str, object]:
    seed = bytes(range(32))
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        private_path = root / "demo_ed25519_private.key"
        public_path = root / "demo_ed25519_public.key"
        private_path.write_bytes(seed)
        signer = Ed25519Signer(private_key_path=private_path, public_key_path=public_path)
        unsigned = ManifestBuilder(session_id="sess-café-01", created_at=CREATED_AT).build()
        signature = signer.sign_manifest(unsigned)
    return {
        "seed_hex": seed.hex(),
        "unsigned_manifest": unsigned,
        "signature": signature,
    }


def signing_input_case() -> dict[str, object]:
    """The two signing inputs over a manifest that already carries both keys.

    daemon/manifest_builder/generator.py:667 signs the whole manifest including
    portable_signature; daemon/verify.py:511 strips only manifest_signature.
    daemon/verify.py:563 strips both for the portable check.
    """
    seed = bytes(range(32))
    with tempfile.TemporaryDirectory() as directory:
        root = pathlib.Path(directory)
        private_path = root / "demo_ed25519_private.key"
        public_path = root / "demo_ed25519_public.key"
        private_path.write_bytes(seed)
        signer = Ed25519Signer(private_key_path=private_path, public_key_path=public_path)
        manifest = ManifestBuilder(session_id="sess-café-01", created_at=CREATED_AT).build()
        manifest["portable_signature"] = signer.sign_manifest(manifest)
        manifest["manifest_signature"] = {
            "algorithm": "HMAC-SHA256",
            "device_id": "local-software",
            "signed_content_hash": "f" * 64,
            "trust_scope": "local_software_integrity",
            "apw:proof_level": "directly_observed",
        }

    portable_input = {
        key: value
        for key, value in manifest.items()
        if key not in {"portable_signature", "manifest_signature"}
    }
    local_input = {key: value for key, value in manifest.items() if key != "manifest_signature"}
    bundle_index = {
        "schema": "apw-evidence-bundle-index-v1",
        "session_id": "sess-café-01",
        "entries": [{"name": "évidence.jsonl", "byte_length": 12, "sha256": "a" * 64}],
        "apw:proof_level": "directly_observed",
    }
    return {
        "manifest": manifest,
        "portable_signing_input": canonical_json_bytes(portable_input).decode("utf-8"),
        "local_signing_input": json.dumps(
            local_input, sort_keys=True, separators=(",", ":")
        ),
        "bundle_index": bundle_index,
        "bundle_index_pretty_sorted": json.dumps(
            bundle_index, indent=2, ensure_ascii=False, sort_keys=True
        )
        + "\n",
    }


def main() -> None:
    FIXTURES.mkdir(parents=True, exist_ok=True)
    for name, payload in (
        ("builder_cases.json", builder_cases()),
        ("schema_cases.json", schema_cases()),
        ("signature_case.json", signature_case()),
        ("signing_input_case.json", signing_input_case()),
    ):
        (FIXTURES / name).write_text(
            json.dumps(payload, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        print(f"wrote {FIXTURES / name}")


if __name__ == "__main__":
    main()
