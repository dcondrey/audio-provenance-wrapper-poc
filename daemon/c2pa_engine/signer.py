from __future__ import annotations

import struct
from dataclasses import dataclass
from pathlib import Path

from c2pa import Builder, C2paSignerInfo, Signer

from daemon.c2pa_engine.manifest import sha256_file

SUPPORTED_ALGORITHMS = frozenset({"es256", "es384", "es512", "ps256", "ps384", "ps512", "ed25519"})

WAV_MIME = "audio/wav"
AIFF_MIME = "audio/aiff"

_WAVE_FORMAT_PCM = 0x0001
_WAVE_FORMAT_IEEE_FLOAT = 0x0003
_WAVE_FORMAT_EXTENSIBLE = 0xFFFE

_FLOAT_EXPORT_ERROR = (
    "32-bit float {container} is not supported for provenance signing because the "
    "association between the observed stems and the export does not survive the float "
    "conversion. Re-export as 16-bit PCM WAV and sign that file."
)


class UnsupportedAssetError(ValueError):
    pass


class SigningError(RuntimeError):
    pass


@dataclass(frozen=True)
class AssetFormat:
    container: str
    mime: str
    embeddable: bool
    sample_format: str
    bit_depth: int | None


@dataclass(frozen=True)
class SigningResult:
    mode: str
    asset_path: Path
    manifest_path: Path | None
    mime: str
    binding: dict[str, object]


def build_signer(cert_chain_pem: bytes, private_key_pem: bytes, alg: str = "es256") -> Signer:
    alg = alg.lower()
    if alg not in SUPPORTED_ALGORITHMS:
        raise SigningError(f"Unsupported signing algorithm {alg!r}")
    if not isinstance(cert_chain_pem, (bytes, bytearray)) or not isinstance(
        private_key_pem, (bytes, bytearray)
    ):
        raise SigningError("Certificate chain and private key must be supplied as PEM bytes")
    chain = bytes(cert_chain_pem)
    if chain.count(b"-----BEGIN CERTIFICATE-----") < 2:
        raise SigningError(
            "Certificate chain must contain the signing leaf followed by at least one CA "
            "certificate; a lone self-signed certificate is rejected by C2PA validation"
        )
    if b"PRIVATE KEY-----" not in bytes(private_key_pem):
        raise SigningError("Private key must be a PEM-encoded private key")
    return Signer.from_info(
        C2paSignerInfo(
            alg=alg.encode("ascii"),
            sign_cert=chain,
            private_key=bytes(private_key_pem),
            ta_url=None,
        )
    )


def _read_head(path: Path, size: int = 16) -> bytes:
    with path.open("rb") as handle:
        return handle.read(size)


def _iter_chunks(path: Path, endian: str):
    """Yield (chunk_id, header_offset, payload_length) over the top-level chunk
    table without loading the audio data."""
    size = path.stat().st_size
    with path.open("rb") as handle:
        offset = 12
        while offset + 8 <= size:
            handle.seek(offset)
            header = handle.read(8)
            if len(header) < 8:
                return
            chunk_id = header[:4]
            (length,) = struct.unpack(endian + "I", header[4:])
            if length > size - offset - 8:
                raise UnsupportedAssetError(
                    f"{path.name}: chunk {chunk_id!r} declares {length} bytes but the file ends first"
                )
            yield chunk_id, offset, length
            offset += 8 + length + (length & 1)


def _find_chunk(path: Path, wanted: bytes, endian: str, max_payload: int = 4096) -> bytes | None:
    for chunk_id, offset, length in _iter_chunks(path, endian):
        if chunk_id == wanted:
            with path.open("rb") as handle:
                handle.seek(offset + 8)
                return handle.read(min(length, max_payload))
    return None


def _manifest_chunk_region(path: Path) -> dict[str, int]:
    """Locate the C2PA chunk actually present in a signed RIFF file.

    IMPORTANT: derived from the chunk table, not from ``dest_size - src_size``.
    c2pa-rs appends at the source length only when the source carries no
    provenance; re-signing a file that already has a C2PA chunk replaces it in
    place, and the size-delta arithmetic then recorded a two-byte exclusion while
    13.6 KB of manifest sat inside the region the binding claims is audio.
    """
    for chunk_id, offset, length in _iter_chunks(path, "<"):
        if chunk_id == b"C2PA":
            return {"start": offset, "length": 8 + length + (length & 1)}
    raise SigningError(
        f"{path.name}: signing reported success but the file carries no C2PA chunk, "
        "so its hard-binding exclusion cannot be recorded"
    )


def _wave_format(path: Path) -> AssetFormat:
    payload = _find_chunk(path, b"fmt ", "<")
    if payload is None:
        raise UnsupportedAssetError(f"{path.name}: no fmt chunk found in RIFF/WAVE file")
    if len(payload) < 16:
        raise UnsupportedAssetError(f"{path.name}: truncated WAV fmt chunk")
    tag, _channels, _rate, _bps, _align, bits = struct.unpack_from("<HHIIHH", payload, 0)
    if tag == _WAVE_FORMAT_EXTENSIBLE:
        if len(payload) < 26:
            raise UnsupportedAssetError(f"{path.name}: truncated WAVE_FORMAT_EXTENSIBLE fmt chunk")
        (tag,) = struct.unpack_from("<H", payload, 24)
    if tag == _WAVE_FORMAT_IEEE_FLOAT:
        raise UnsupportedAssetError(_FLOAT_EXPORT_ERROR.format(container="WAV"))
    if tag != _WAVE_FORMAT_PCM:
        raise UnsupportedAssetError(
            f"{path.name}: WAV format tag 0x{tag:04x} is not supported; "
            "export 16-bit PCM WAV"
        )
    return AssetFormat("wav", WAV_MIME, True, "pcm", bits)


def _aiff_format(path: Path) -> AssetFormat:
    kind = _read_head(path)[8:12]
    payload = _find_chunk(path, b"COMM", ">")
    if payload is None:
        raise UnsupportedAssetError(f"{path.name}: no COMM chunk found in AIFF file")
    if len(payload) < 18:
        raise UnsupportedAssetError(f"{path.name}: truncated AIFF COMM chunk")
    (bits,) = struct.unpack_from(">h", payload, 6)
    compression = payload[18:22] if kind == b"AIFC" else b"NONE"
    if compression.lower() in {b"fl32", b"fl64"}:
        raise UnsupportedAssetError(_FLOAT_EXPORT_ERROR.format(container="AIFF"))
    return AssetFormat("aiff", AIFF_MIME, False, "pcm", bits)


def detect_format(path: Path | str) -> AssetFormat:
    path = Path(path)
    head = _read_head(path, 16)
    if head[:4] == b"RIFF" and head[8:12] == b"WAVE":
        return _wave_format(path)
    if head[:4] == b"FORM" and head[8:12] in {b"AIFF", b"AIFC"}:
        return _aiff_format(path)
    raise UnsupportedAssetError(
        f"{path.name}: unrecognised audio container; the engine signs 16-bit PCM WAV "
        "(embedded) and AIFF (sidecar)"
    )


def sign_wav(
    src: Path | str,
    dest: Path | str,
    manifest: dict[str, object],
    signer: Signer,
) -> SigningResult:
    src = Path(src)
    dest = Path(dest)
    asset = detect_format(src)
    if not asset.embeddable:
        raise UnsupportedAssetError(
            f"{src.name}: {asset.container} cannot carry an embedded manifest; use sign_sidecar"
        )
    dest.parent.mkdir(parents=True, exist_ok=True)
    try:
        Builder(manifest).sign_file(src, dest, signer)
    except Exception as exc:
        raise SigningError(f"Embedded signing of {src.name} failed: {exc}") from exc
    return SigningResult(
        mode="embedded",
        asset_path=dest,
        manifest_path=dest,
        mime=asset.mime,
        binding={
            "type": "c2pa.hash.data",
            "algorithm": "sha256",
            "covers": (
                "every byte of the signed asset except the C2PA manifest chunk and the RIFF "
                "size field, which is rewritten to count that chunk"
            ),
            "excluded_region": _manifest_chunk_region(dest),
            "rewritten_header_field": {"start": 4, "length": 4},
            "container_rewrite_tolerance": "manifest chunk only",
            "source_sha256": sha256_file(src),
        },
    )


def sign_sidecar(
    src: Path | str,
    dest_manifest_path: Path | str,
    manifest: dict[str, object],
    signer: Signer,
    mime: str | None = None,
) -> SigningResult:
    src = Path(src)
    dest_manifest_path = Path(dest_manifest_path)
    asset = detect_format(src)
    builder = Builder(manifest)
    builder.set_no_embed()
    try:
        with src.open("rb") as handle:
            manifest_bytes = builder.sign(signer, mime or asset.mime, handle)
    except Exception as exc:
        raise SigningError(f"Sidecar signing of {src.name} failed: {exc}") from exc
    dest_manifest_path.parent.mkdir(parents=True, exist_ok=True)
    dest_manifest_path.write_bytes(manifest_bytes)
    return SigningResult(
        mode="sidecar",
        asset_path=src,
        manifest_path=dest_manifest_path,
        mime=mime or asset.mime,
        binding={
            "type": "c2pa.hash.data",
            "algorithm": "sha256",
            "covers": "whole file",
            "exclusions": [],
            "container_rewrite_tolerance": "none",
            "source_sha256": sha256_file(src),
            "note": (
                "The sidecar binding hashes every byte of the asset with no exclusions, so "
                "any container rewrite, tag edit or re-mux breaks it."
            ),
        },
    )


def sign_asset(
    src: Path | str,
    dest: Path | str,
    manifest: dict[str, object],
    signer: Signer,
    sidecar_path: Path | str | None = None,
) -> SigningResult:
    src = Path(src)
    asset = detect_format(src)
    if asset.embeddable:
        return sign_wav(src, dest, manifest, signer)
    sidecar = Path(sidecar_path) if sidecar_path is not None else Path(dest).with_suffix(".c2pa")
    return sign_sidecar(src, sidecar, manifest, signer, mime=asset.mime)
