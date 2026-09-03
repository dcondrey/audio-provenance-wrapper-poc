#!/usr/bin/env python3
"""Assemble a curated, sendable evidence package from one completed demo session."""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
import zipfile
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

PROJECT_ROOT = Path(__file__).resolve().parent.parent


def session_manifests(session_dir: Path) -> list[Path]:
    return sorted(
        p
        for p in (session_dir / "manifests").glob("*_manifest.json")
        if "altered" not in p.name
    )


def find_manifest(session_dir: Path, selected: Path | None = None) -> Path:
    manifests = session_manifests(session_dir)
    if not manifests:
        raise SystemExit(f"no manifest in {session_dir / 'manifests'}")
    if selected is not None:
        chosen = selected if selected.is_absolute() else session_dir / "manifests" / selected.name
        chosen = chosen.resolve()
        if chosen not in {m.resolve() for m in manifests}:
            listing = "\n".join(f"  {m.name}" for m in manifests)
            raise SystemExit(
                f"{chosen} is not one of the manifests in {session_dir / 'manifests'}:\n{listing}"
            )
        return chosen
    if len(manifests) > 1:
        newest = max(manifests, key=lambda m: m.stat().st_mtime)
        print(
            f"{len(manifests)} manifests in {session_dir / 'manifests'}; packaging the most "
            f"recent one: {newest.name}. Pass --manifest to choose another.",
            file=sys.stderr,
        )
        return newest
    return manifests[0]


def optional_number(value: object, spec: str) -> str:
    """Render a manifest field that is null whenever its status is unavailable."""
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return format(value, spec)
    return "n/a"


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


@dataclass(frozen=True)
class PackagedPrimaries:
    manifest_rel: str
    export_rel: str | None
    evidence_rel: tuple[str, ...]
    chain_rel: tuple[str, ...]
    derived_audio: tuple[tuple[str, str], ...]


def sha256_file(path: Path, byte_length: int | None = None) -> str:
    digest = hashlib.sha256()
    remaining = byte_length
    with path.open("rb") as handle:
        while remaining is None or remaining > 0:
            chunk = handle.read(1 << 20 if remaining is None else min(1 << 20, remaining))
            if not chunk:
                break
            digest.update(chunk)
            if remaining is not None:
                remaining -= len(chunk)
    if remaining is not None and remaining > 0:
        raise SystemExit(f"{path} is shorter than the {byte_length} bytes the manifest binds")
    return digest.hexdigest()


def copy_primaries(manifest_data: dict, manifest_rel: str, dest: Path) -> PackagedPrimaries:
    """Copy the primary artefacts the manifest binds into the package.

    IMPORTANT: the manifest is signed, so its absolute session paths are never
    rewritten. The package carries the bytes and the README supplies the
    overrides that let a verifier reach them.
    """
    export = manifest_data["export"]
    export_rel: str | None = None
    source = Path(str(export.get("file_path", "")))
    if source.is_file():
        target = dest / "export" / export["file_name"]
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        actual = sha256_file(target)
        if actual != export["sha256"]:
            raise SystemExit(
                f"{source} hashes to {actual}, but the manifest binds {export['sha256']}. "
                "Refusing to ship an export that does not match its own manifest."
            )
        export_rel = f"export/{export['file_name']}"

    binding = manifest_data.get("evidence_binding") or {}
    evidence_dir = Path(str(binding.get("evidence_directory", "")))
    bindings = binding.get("evidence_files") or {
        name: {"sha256": digest} for name, digest in (binding.get("evidence_file_hashes") or {}).items()
    }
    evidence_rel: list[str] = []
    chain_rel: list[str] = []
    for name, record in sorted(bindings.items()):
        source = evidence_dir / str(name)
        if not source.is_file():
            continue
        target = dest / "evidence" / str(name)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
        byte_length = record.get("byte_length") if isinstance(record, dict) else None
        actual = sha256_file(target, byte_length if isinstance(byte_length, int) else None)
        expected = record.get("sha256") if isinstance(record, dict) else None
        if expected and actual != expected:
            raise SystemExit(
                f"{source} does not match the bound evidence hash for {name} "
                f"({actual} != {expected}). Refusing to ship inconsistent evidence."
            )
        evidence_rel.append(f"evidence/{name}")
        # Session-lifecycle logs are bound too, but carry no buffer_hash chain,
        # so the chain verifier would honestly fail on them.
        if b'"buffer_hash"' in target.read_bytes():
            chain_rel.append(f"evidence/{name}")

    derived: list[tuple[str, str]] = []
    presentation = manifest_data.get("presentation") or {}
    for key in ("c2pa_signed_asset", "c2pa_sidecar_manifest"):
        rel = presentation.get(key)
        if not rel:
            continue
        packaged = dest / str(rel)
        if packaged.is_file():
            derived.append((str(rel), sha256_file(packaged)))

    return PackagedPrimaries(
        manifest_rel=manifest_rel,
        export_rel=export_rel,
        evidence_rel=tuple(evidence_rel),
        chain_rel=tuple(chain_rel),
        derived_audio=tuple(derived),
    )


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
    packaged: "PackagedPrimaries",
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
    offset = optional_number(association.get("best_offset_seconds"), ".3f")
    lines.append(
        f"| stem–export association | {association['status']} "
        f"(confidence {optional_number(association.get('confidence'), '.2f')}, "
        f"coverage {optional_number(association.get('matched_coverage'), '.2f')}, "
        f"offset {offset if offset == 'n/a' else offset + 's'}) | "
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
    if packaged.export_rel:
        lines.append(
            f"  Shipped verbatim at `{packaged.export_rel}`; `shasum -a 256` on it "
            "reproduces the digest above."
        )
    else:
        lines.append(
            "  The exported audio was NOT reachable at packaging time, so it is not in "
            "this package and the digest above cannot be reproduced from these files alone."
        )
    for rel, digest in packaged.derived_audio:
        lines.append(
            f"- Derived signed asset: `{rel}`, `sha256:{digest}` — a **different** digest "
            "on purpose. C2PA embedding appends a manifest chunk after the last original "
            "audio byte, so the signed copy cannot hash the same as the export. The "
            "manifest binds the export digest; the appended region is excluded from the "
            "C2PA hard binding. Neither digest disagreeing with the other is tampering."
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
        "From a checkout of the repository (Python ≥ 3.11 with `cryptography` installed). "
        "`PKG` is the directory holding this README; every path below resolves inside it, "
        "so the commands work wherever the package is unpacked:"
    )
    lines.append("")
    lines.append("```sh")
    lines.append("PKG=<path-to-this-directory>")
    manifest_cmd = f'./scripts/verify_demo.sh "$PKG/{packaged.manifest_rel}" --public-only'
    if packaged.export_rel:
        manifest_cmd += f' \\\n    --export "$PKG/{packaged.export_rel}"'
    lines.append(manifest_cmd)
    for rel in packaged.chain_rel:
        lines.append(f'./scripts/verify_demo.sh "$PKG/{rel}"')
    lines.append("```")
    lines.append("")
    lines.append(
        "The first command re-hashes the manifest, its Ed25519 portable signature and the "
        "exported audio shipped here. Each further command replays the hash chain in one "
        "evidence file end to end."
    )
    lines.append("")
    lines.append(
        "The manifest records the author's absolute session paths, which do not exist on "
        "your machine. `--export` overrides the export path; the evidence files are "
        "verified directly by the commands above instead. Without those overrides the "
        "verifier reports `export_file_unavailable` / `evidence_file_unavailable` "
        "warnings — an absence of input, never a tamper finding."
    )
    lines.append("")
    lines.append(
        "The evidence bundle ZIP is deterministic; `artifacts/*_bundle_index.json` is the "
        "Ed25519-signed root that commits to every file in it. The signer identity is "
        "self-generated and stated as `not_established` in-band; binding it to a verified "
        "identity is exactly the downstream registration seam this adapter leaves open."
    )
    lines.append("")
    return "\n".join(lines)


def verify_packaged_copy(package_dir: Path, packaged: PackagedPrimaries) -> tuple[int, str]:
    """Run the exact commands the README prints, against the package's own files."""
    parts: list[str] = []
    worst = 0
    extra: list[str] = ["--public-only"]
    if packaged.export_rel:
        extra += ["--export", str(package_dir / packaged.export_rel)]
    rc, transcript = run_verifier(package_dir / packaged.manifest_rel, *extra)
    worst = max(worst, rc)
    parts.append(f"$ daemon.verify {packaged.manifest_rel} {' '.join(extra)}\n{transcript}")
    for rel in packaged.chain_rel:
        _, transcript = run_verifier(package_dir / rel)
        # A chain file carries no manifest, so the verifier's manifest-level checks are
        # reported unrun and it exits non-zero. The claim under test here is narrower:
        # the hash chain in this file replays end to end.
        if "chain_intact" not in transcript:
            worst = max(worst, 1)
        parts.append(f"$ daemon.verify {rel}\n{transcript}")
    return worst, "\n".join(parts)


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
    parser.add_argument(
        "--manifest",
        type=Path,
        default=None,
        help="Manifest to package when the session holds more than one. Defaults to the newest.",
    )
    args = parser.parse_args()

    session_dir = args.session_dir.resolve()
    manifest_path = find_manifest(session_dir, args.manifest)
    manifest_data = json.loads(manifest_path.read_text())
    stem = manifest_path.name.removesuffix("_manifest.json")
    verification_path = manifest_path.parent / "artifacts" / f"{stem}_verification.json"
    if not verification_path.is_file():
        raise SystemExit(
            f"{manifest_path.name} has no verifier result at {verification_path}. "
            "The session was not sealed completely; re-export or pass --manifest."
        )
    verification = json.loads(verification_path.read_text())
    session_kind = (
        "synthetic" if manifest_data["session_id"].startswith("synthetic-") else "capture"
    )

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    package_dir = args.output.resolve() / f"evidence-package-{stamp}"
    if package_dir.exists():
        raise SystemExit(f"refusing to overwrite {package_dir}")
    copy_manifest_tree(manifest_path, package_dir)
    packaged = copy_primaries(manifest_data, manifest_path.name, package_dir)

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
                copy_primaries(
                    null_manifest, null_manifest_path.name, package_dir / "honest-null"
                )

    contents = sorted(
        [
            str(p.relative_to(package_dir))
            for p in package_dir.rglob("*")
            if p.is_file()
        ]
        + ["README.md", "portable-verification-transcript.txt"]
    )
    readme = build_readme(
        manifest_data, verification, session_kind, tamper_summary, null_manifest,
        contents, packaged,
    )
    (package_dir / "README.md").write_text(readme)

    portable_rc, portable_transcript = verify_packaged_copy(package_dir, packaged)
    (package_dir / "portable-verification-transcript.txt").write_text(portable_transcript)
    if portable_rc != 0:
        raise SystemExit(
            "the package does not verify from its own files:\n" + portable_transcript
        )

    zip_path = package_dir.with_suffix(".zip")
    deterministic_zip(package_dir, zip_path)

    print(json.dumps({
        "package_dir": str(package_dir),
        "package_zip": str(zip_path),
        "session_kind": session_kind,
        "verifier_outcome": verification["outcome"],
        "packaged_export": packaged.export_rel,
        "packaged_evidence": list(packaged.evidence_rel),
        "packaged_hash_chains": list(packaged.chain_rel),
        "files": len(contents),
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
