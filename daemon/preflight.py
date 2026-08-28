from __future__ import annotations

import argparse
import shutil
import socket
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from daemon.signing import Ed25519Signer


@dataclass(frozen=True)
class Check:
    name: str
    status: str
    detail: str


def run_preflight(session_dir: Path, port: int = 9876) -> list[Check]:
    checks: list[Check] = []
    session_dir = session_dir.expanduser().resolve()
    for name in ("evidence", "samples", "exports", "manifests"):
        path = session_dir / name
        try:
            path.mkdir(parents=True, exist_ok=True)
            checks.append(Check(f"directory:{name}", "ok", str(path)))
        except OSError as exc:
            checks.append(Check(f"directory:{name}", "fail", str(exc)))

    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        probe.bind(("127.0.0.1", port))
    except OSError as exc:
        checks.append(Check("udp_port", "fail", f"127.0.0.1:{port} unavailable: {exc}"))
    else:
        checks.append(Check("udp_port", "ok", f"127.0.0.1:{port} available"))
    finally:
        probe.close()

    free_bytes = shutil.disk_usage(session_dir).free
    checks.append(Check(
        "disk_space",
        "ok" if free_bytes >= 2 * 1024**3 else "fail",
        f"{free_bytes / 1024**3:.1f} GiB free (2 GiB minimum)",
    ))
    checks.append(Check(
        "python_runtime",
        "ok" if sys.version_info >= (3, 11) else "fail",
        sys.version.split()[0],
    ))

    try:
        signer = Ed25519Signer()
        public_key = signer.public_key_hex()
    except Exception as exc:
        checks.append(Check("signing_material", "fail", str(exc)))
    else:
        checks.append(Check(
            "signing_material", "ok",
            f"Ed25519 public key {public_key[:16]}…; self-generated identity is unverified",
        ))

    project_root = Path(__file__).resolve().parent.parent
    built = project_root / "build/AudioProvenanceCapture_artefacts/Release/VST3/Audio Provenance Capture.vst3"
    installed = Path.home() / "Library/Audio/Plug-Ins/VST3/Audio Provenance Capture.vst3"
    bundles = [path for path in (installed, built) if path.is_dir()]
    if bundles:
        verification = subprocess.run(
            ["codesign", "--verify", "--deep", "--strict", str(bundles[0])],
            check=False,
            capture_output=True,
            text=True,
        )
        checks.append(Check(
            "plugin_bundle",
            "ok" if verification.returncode == 0 else "fail",
            str(bundles[0]) if verification.returncode == 0 else verification.stderr.strip(),
        ))
    else:
        checks.append(Check(
            "plugin_bundle", "fail",
            "Release VST3 not found; run ./scripts/build_plugin.sh --install",
        ))

    ableton_apps = sorted(Path("/Applications").glob("Ableton Live*.app"))
    checks.append(Check(
        "ableton_runtime",
        "ok" if ableton_apps else "warn",
        str(ableton_apps[-1]) if ableton_apps else "Ableton app not found in /Applications; automated rehearsal remains available",
    ))
    return checks


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Preflight the routed-audio evidence demo.")
    parser.add_argument("session_dir", type=Path)
    parser.add_argument("--port", type=int, default=9876)
    args = parser.parse_args(argv)
    checks = run_preflight(args.session_dir, args.port)
    for check in checks:
        print(f"{check.status.upper():4}  {check.name:20} {check.detail}")
    failures = [check for check in checks if check.status == "fail"]
    print(f"\nPreflight: {'READY' if not failures else 'BLOCKED'} ({len(failures)} failures)")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
