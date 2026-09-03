from __future__ import annotations

import argparse
import shutil
import socket
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

from daemon.signing import Ed25519Signer


MINIMUM_PYTHON = (3, 11)
REQUIRED_ARCHITECTURES = frozenset({"x86_64", "arm64"})


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
    runtime_ok = sys.version_info >= MINIMUM_PYTHON
    checks.append(Check(
        "python_runtime",
        "ok" if runtime_ok else "fail",
        f"{sys.version.split()[0]} at {sys.executable}" + (
            "" if runtime_ok else
            f"; Python {MINIMUM_PYTHON[0]}.{MINIMUM_PYTHON[1]}+ is required. Run the repo venv "
            "interpreter (./.venv/bin/python -m daemon.preflight), or create it with "
            "'uv venv && uv pip install -r requirements.txt'."
        ),
    ))

    try:
        signer = Ed25519Signer()
        public_key = signer.public_key_hex()
    except Exception as exc:
        checks.append(Check(
            "signing_material",
            "fail",
            f"{type(exc).__name__}: {exc}. Install the daemon dependencies into the repo venv: "
            "'uv pip install -r requirements.txt', then rerun with ./.venv/bin/python.",
        ))
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
        binary = bundles[0] / "Contents/MacOS/Audio Provenance Capture"
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
        # IMPORTANT: file(1) exits 0 for any readable Mach-O, so keying on its
        # status computed nothing about architecture: a thin arm64 bundle passed
        # on the Intel Mac the universal build exists to serve.
        architecture = subprocess.run(
            ["lipo", "-archs", str(binary)], check=False, capture_output=True, text=True,
        )
        slices = set(architecture.stdout.split()) if architecture.returncode == 0 else set()
        missing = sorted(REQUIRED_ARCHITECTURES - slices)
        checks.append(Check(
            "plugin_architecture",
            "ok" if not missing else "fail",
            " ".join(sorted(slices)) if not missing else (
                f"missing {', '.join(missing)} (found {' '.join(sorted(slices)) or 'nothing'}); "
                "rebuild universal with ./scripts/build_plugin.sh --install"
            ),
        ))
        signing = subprocess.run(
            ["codesign", "-dvvv", str(bundles[0])],
            check=False,
            capture_output=True,
            text=True,
        )
        signing_lines = [
            line for line in signing.stderr.splitlines()
            if line.startswith(("Authority=", "Signature=", "TeamIdentifier=", "Runtime Version="))
        ]
        checks.append(Check(
            "plugin_signing",
            "ok" if signing.returncode == 0 else "fail",
            "; ".join(signing_lines) or signing.stderr.strip(),
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
