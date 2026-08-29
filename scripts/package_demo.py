#!/usr/bin/env python3
"""Assemble a curated, sendable evidence package from one completed demo session."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import zipfile
from datetime import datetime, timezone
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent


def find_manifest(session_dir: Path) -> Path:
    manifests = sorted(
        p
        for p in (session_dir / "manifests").glob("*_manifest.json")
        if "altered" not in p.name
    )
    if len(manifests) != 1:
        raise SystemExit(
            f"expected exactly one manifest in {session_dir / 'manifests'}, found {len(manifests)}"
        )
    return manifests[0]


def run_verifier(manifest: Path, *extra: str) -> tuple[int, str]:
    proc = subprocess.run(
        [sys.executable, "-m", "daemon.verify", str(manifest), *extra],
        cwd=PROJECT_ROOT,
        capture_output=True,
        text=True,
    )
    return proc.returncode, proc.stdout + proc.stderr


def latest_adversarial_dir(session_dir: Path) -> Path | None:
    root = session_dir / "adversarial-copies"
    if not root.is_dir():
        return None
    stamps = sorted(d for d in root.iterdir() if d.is_dir())
    return stamps[-1] if stamps else None


def copy_manifest_tree(manifest: Path, dest: Path) -> None:
    src = manifest.parent
    dest.mkdir(parents=True, exist_ok=True)
    stem = manifest.name.removesuffix("_manifest.json")
    shutil.copy2(manifest, dest / manifest.name)
    fight_card = src / f"{stem}_provenance.html"
    if fight_card.exists():
        shutil.copy2(fight_card, dest / fight_card.name)
    artifacts = src / "artifacts"
    if artifacts.is_dir():
        shutil.copytree(artifacts, dest / "artifacts", dirs_exist_ok=True)


def proof_label(level: str) -> str:
    return level.replace("_", " ")


def git_commit() -> str:
    proc = subprocess.run(
        ["git", "rev-parse", "--short", "HEAD"],
        cwd=PROJECT_ROOT,
        capture_output=True,
        text=True,
    )
    return proc.stdout.strip() if proc.returncode == 0 else "unknown"


def build_readme(
    manifest_data: dict,
    verification: dict,
    session_kind: str,
    tamper_summary: list[str],
    null_manifest: dict | None,
    contents: list[str],
) -> str:
    export = manifest_data["export"]
    coverage = manifest_data["observation_coverage"]
    association = manifest_data["stem_export_association"]
    ack = manifest_data["daemon_receipt_acknowledgement"]
    binding = manifest_data["evidence_binding"]
    unobserved = manifest_data.get("apw:unobserved", [])
    generated = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC")

    lines: list[str] = []
    lines.append("# Routed Audio Evidence Adapter — demo evidence package")
    lines.append("")
    lines.append(
        f"Generated {generated} from session `{manifest_data['session_id']}` "
        f"at commit `{git_commit()}`."
    )
    lines.append("")
    if session_kind == "synthetic":
        lines.append(
            "**Session type: synthetic rehearsal.** Deterministic generated audio drove "
            "the same daemon, UDP event protocol, hash chain, manifest builder, evidence "
            "bundler, and verifier that the live Ableton VST3 path uses. No claim in this "
            "package rests on the audio being musically meaningful; the claims are about "
            "the evidence chain."
        )
    else:
        lines.append(
            "**Session type: live capture.** A real DAW session drove the VST3 plug-in; "
            "the daemon observed, acknowledged, and sealed the evidence below."
        )
    lines.append("")
    lines.append(f"> {manifest_data['core_principle']}")
    lines.append("")
    lines.append("## The chain this package demonstrates")
    lines.append("")
    lines.append("| Step | Result | Proof level |")
    lines.append("|---|---|---|")
    for claim in manifest_data["claim_summary"]:
        lines.append(
            f"| {claim['claim'].replace('_', ' ')} | {claim['value']} | "
            f"{proof_label(claim['apw:proof_level'])} |"
        )
    lines.append(
        f"| export hashed | `sha256:{export['sha256'][:16]}…` | "
        f"{proof_label(export['apw:proof_level'])} |"
    )
    lines.append(
        f"| stem–export association | {association['status']} "
        f"(confidence {association['confidence']:.2f}, "
        f"coverage {association['matched_coverage']:.2f}, "
        f"offset {association['best_offset_seconds']:.3f}s) | "
        f"{proof_label(association['apw:proof_level'])} |"
    )
    lines.append("")
    lines.append("## Headline numbers")
    lines.append("")
    lines.append(f"- Hash-chained observation windows: {binding['chain_length']}")
    lines.append(f"- Chain head: `{binding['last_window_hash'][:32]}…`")
    lines.append(
        f"- Export: `{export['file_name']}`, {export['file_size_bytes']:,} bytes, "
        f"`sha256:{export['sha256']}`"
    )
    lines.append(
        f"- Coverage: `{coverage['status']}` — {coverage['basis']}"
    )
    lines.append(
        f"- Daemon receipt acknowledgement: {ack['status']} "
        f"({ack['counters'].get('sent', 'n/a')} ACK packets dispatched). {ack['scope']}"
    )
    lines.append(f"- Verifier outcome: **{verification['outcome']}** — {verification['qualified_scope']}")
    lines.append("")
    lines.append("## What is deliberately NOT claimed")
    lines.append("")
    lines.append(
        "The association above is labelled *inferred*, not proven. The following remain "
        "unestablished and the manifest says so in-band:"
    )
    lines.append("")
    for item in unobserved:
        if isinstance(item, str):
            lines.append(f"- {item}")
        elif isinstance(item, dict):
            lines.append(f"- {item.get('claim', json.dumps(item))}")
    lines.append("")
    if tamper_summary:
        lines.append("## Tamper demonstration")
        lines.append("")
        lines.extend(tamper_summary)
        lines.append("")
    if null_manifest is not None:
        null_assoc = null_manifest["stem_export_association"]
        null_cov = null_manifest["observation_coverage"]
        lines.append("## Honest null (`honest-null/`)")
        lines.append("")
        lines.append(
            "An export hashed with **no** observation events present yields file integrity "
            f"`verified` but coverage `{null_cov['status']}` and association "
            f"`{null_assoc['status']}`. The system does not claim the audio is synthetic, "
            "AI-generated, or unauthorised; it reports only that nothing was observed."
        )
        lines.append("")
    lines.append("## Contents")
    lines.append("")
    for entry in contents:
        lines.append(f"- `{entry}`")
    lines.append("")
    lines.append("## Independent verification")
    lines.append("")
    lines.append(
        "From a checkout of the repository (Python ≥ 3.11 with `cryptography` installed):"
    )
    lines.append("")
    lines.append("```sh")
    lines.append("./scripts/verify_demo.sh <path-to>/" + export["file_name"].rsplit(".", 1)[0] + "_manifest.json --public-only")
    lines.append("```")
    lines.append("")
    lines.append(
        "The evidence bundle ZIP is deterministic; `artifacts/*_bundle_index.json` is the "
        "Ed25519-signed root that commits to every file in it. The signer identity is "
        "self-generated and stated as `not_established` in-band; binding it to a verified "
        "identity is exactly the downstream registration seam this adapter leaves open."
    )
    lines.append("")
    return "\n".join(lines)


def deterministic_zip(src_dir: Path, zip_path: Path) -> None:
    files = sorted(p for p in src_dir.rglob("*") if p.is_file())
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in files:
            info = zipfile.ZipInfo(
                str(path.relative_to(src_dir)), date_time=(1980, 1, 1, 0, 0, 0)
            )
            # ZipInfo defaults to ZIP_STORED regardless of the archive default.
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, path.read_bytes())


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("session_dir", type=Path)
    parser.add_argument("--output", type=Path, default=Path("demo-output/founder-package"))
    args = parser.parse_args()

    session_dir = args.session_dir.resolve()
    manifest_path = find_manifest(session_dir)
    manifest_data = json.loads(manifest_path.read_text())
    stem = manifest_path.name.removesuffix("_manifest.json")
    verification_path = manifest_path.parent / "artifacts" / f"{stem}_verification.json"
    verification = json.loads(verification_path.read_text())
    session_kind = (
        "synthetic" if manifest_data["session_id"].startswith("synthetic-") else "capture"
    )

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    package_dir = args.output.resolve() / f"evidence-package-{stamp}"
    if package_dir.exists():
        raise SystemExit(f"refusing to overwrite {package_dir}")
    copy_manifest_tree(manifest_path, package_dir)

    rc, transcript = run_verifier(manifest_path)
    if rc != 0:
        raise SystemExit(f"original manifest failed verification:\n{transcript}")
    (package_dir / "verification-transcript.txt").write_text(transcript)

    tamper_summary: list[str] = []
    null_manifest: dict | None = None
    adversarial = latest_adversarial_dir(session_dir)
    if adversarial is not None:
        altered_manifest = adversarial / "altered-manifest.json"
        if altered_manifest.exists():
            rc, transcript = run_verifier(altered_manifest, "--public-only")
            if rc == 0:
                raise SystemExit("altered manifest unexpectedly verified")
            (package_dir / "tamper-altered-manifest-transcript.txt").write_text(transcript)
            tamper_summary.append(
                "- A copy of the manifest with one sentence changed fails verification "
                "(`tamper-altered-manifest-transcript.txt`: outcome `changed`, content "
                "hash and portable signature both flagged)."
            )
        altered_exports = sorted(adversarial.glob("altered-export.*"))
        if altered_exports:
            rc, transcript = run_verifier(
                manifest_path, "--public-only", "--export", str(altered_exports[0])
            )
            if rc == 0:
                raise SystemExit("altered export unexpectedly verified")
            (package_dir / "tamper-altered-export-transcript.txt").write_text(transcript)
            tamper_summary.append(
                "- A copy of the exported audio with bytes appended fails hash "
                "verification against the sealed manifest "
                "(`tamper-altered-export-transcript.txt`)."
            )
        export_only_root = adversarial / "export-only"
        if export_only_root.is_dir():
            null_sessions = sorted(d for d in export_only_root.iterdir() if d.is_dir())
            if null_sessions:
                null_manifest_path = find_manifest(null_sessions[-1])
                null_manifest = json.loads(null_manifest_path.read_text())
                copy_manifest_tree(null_manifest_path, package_dir / "honest-null")

    contents = sorted(
        str(p.relative_to(package_dir))
        for p in package_dir.rglob("*")
        if p.is_file()
    )
    contents.insert(0, "README.md")
    readme = build_readme(
        manifest_data, verification, session_kind, tamper_summary, null_manifest, contents
    )
    (package_dir / "README.md").write_text(readme)

    zip_path = package_dir.with_suffix(".zip")
    deterministic_zip(package_dir, zip_path)

    print(json.dumps({
        "package_dir": str(package_dir),
        "package_zip": str(zip_path),
        "session_kind": session_kind,
        "verifier_outcome": verification["outcome"],
        "files": len(contents),
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
