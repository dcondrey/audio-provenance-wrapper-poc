import sys
import tempfile
import wave
from pathlib import Path


def _self_test() -> int:
    import platform

    from daemon.signing import Ed25519Signer, verify_ed25519_signature

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        signer = Ed25519Signer(root / "private.key", root / "public.key")
        manifest = {"apw:proof_level": "directly_observed", "self_test": True}
        signature = signer.sign_manifest(manifest)
        # REQUIRED: verify against the key this test just created, not the machine's
        # pinned default, or the result depends on ~/.apw rather than on the runtime.
        accepted, reason = verify_ed25519_signature(
            manifest, signature, public_key_path=root / "public.key"
        )

    machine = platform.machine()
    if not accepted:
        print(f"self-test FAILED on {machine}: {reason}", file=sys.stderr)
        return 1
    print(f"self-test OK on {machine}: Ed25519 sign+verify through cryptography")
    return 0


def _c2pa_self_test() -> int:
    """Drive the real C2PA sign+verify path this runtime would use on an export.

    REQUIRED: the C2PA engine is imported lazily during sealing, so a frozen daemon
    missing it still starts, still answers --help, and only fails at the moment an
    export is sealed. This arm is the packaging gate that refuses to ship that.
    """
    import platform

    machine = platform.machine()
    try:
        from daemon.c2pa_engine.manifest import ManifestSpec, build_manifest
        from daemon.c2pa_engine.signer import build_signer, detect_format, sign_asset
        from daemon.c2pa_engine.verifier import verify_asset
        from daemon.provenance import LocalReferenceProvider, VerificationState
    except ImportError as exc:
        print(f"c2pa-self-test FAILED on {machine}: {exc}", file=sys.stderr)
        return 1

    try:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "self-test.wav"
            with wave.open(str(source), "wb") as handle:
                handle.setnchannels(1)
                handle.setsampwidth(2)
                handle.setframerate(44100)
                handle.writeframes(b"\x00\x10" * 4410)

            provider = LocalReferenceProvider(root / "provenance")
            material = provider.issue_signing_material()
            signer = build_signer(
                material.certificate_chain_pem,
                material.private_key_handle,
                material.algorithm,
            )
            asset = detect_format(source)
            signing = sign_asset(
                source,
                root / "self-test_c2pa.wav",
                build_manifest(ManifestSpec(title=source.name, mime=asset.mime)),
                signer,
            )
            result = verify_asset(
                signing.asset_path,
                signing.mime,
                trust_anchors_pem=material.trust_anchor_pem,
            )
    except Exception as exc:  # the frozen runtime is the thing under test
        print(f"c2pa-self-test FAILED on {machine}: {type(exc).__name__}: {exc}", file=sys.stderr)
        return 1

    if result.state != VerificationState.VERIFIED.value or result.failure_codes:
        print(
            f"c2pa-self-test FAILED on {machine}: state={result.state} "
            f"failures={result.failure_codes or ()} {result.detail}",
            file=sys.stderr,
        )
        return 1
    print(f"c2pa-self-test OK on {machine}: signed and verified a WAV against its own root")
    return 0


def _module_fingerprint() -> int:
    """Print a structural digest of the daemon package as this runtime sees it.

    IMPORTANT: PyInstaller froze a stale __pycache__ copy of one module once, producing
    a signed daemon that died at startup on a method the source had. Comparing this
    digest between the frozen binary and the repository is what detects that.
    """
    import hashlib
    import importlib
    import inspect
    import pkgutil

    import daemon

    digest = hashlib.sha256()
    names = sorted(
        info.name for info in pkgutil.walk_packages(daemon.__path__, "daemon.")
    )
    for name in ["daemon", *names]:
        try:
            module = importlib.import_module(name)
        except Exception as exc:
            print(f"module-fingerprint FAILED importing {name}: {exc}", file=sys.stderr)
            return 1
        digest.update(name.encode())
        for attr in sorted(dir(module)):
            digest.update(attr.encode())
            member = getattr(module, attr, None)
            if inspect.isclass(member) and member.__module__ == name:
                for method in sorted(dir(member)):
                    digest.update(method.encode())
    print(digest.hexdigest()[:32])
    return 0


if __name__ == "__main__":
    argv = sys.argv[1:]
    if argv[:1] == ["--module-fingerprint"]:
        raise SystemExit(_module_fingerprint())
    if argv[:1] == ["--self-test"]:
        raise SystemExit(_self_test())
    if argv[:1] == ["--c2pa-self-test"]:
        raise SystemExit(_c2pa_self_test())

    from daemon.__main__ import main

    raise SystemExit(main(argv))
