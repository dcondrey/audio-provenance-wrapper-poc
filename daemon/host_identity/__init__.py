"""Host identification for hosts the plug-in wrapper does not recognise.

The wrapper's own recognition (juce::PluginHostType) is authoritative and is
never overridden. For an unrecognised host, the executable file name is looked
up in data/host_executables.json by exact, case-insensitive equality. A table
hit is an inference from a name, not an observation of the host.
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

SCHEMA_VERSION = 1
MAX_TABLE_BYTES = 256 * 1024
MAX_HOSTS = 512
MAX_MATCHES_PER_HOST = 32
MAX_STRING = 512
PLATFORMS = ("windows", "macos", "linux")
DEFAULT_TABLE_PATH = Path(__file__).resolve().parents[2] / "data" / "host_executables.json"

IDENT_JUCE = "juce_plugin_host_type"
IDENT_INFERRED = "inferred_from_executable_name"
IDENT_NONE = "unrecognised"


class HostTableError(ValueError):
    """The host executable table is malformed or over a bound."""


@dataclass(frozen=True)
class HostIdentity:
    recognised: bool
    host_name: str | None
    identification: str
    proof_level: str
    host_id: str | None = None
    display_name: str | None = None
    source_url: str | None = None


def normalise_executable_name(name: str) -> str:
    """Casefold and drop one trailing '.exe'. Nothing else is altered."""
    folded = name.strip().casefold()
    if folded.endswith(".exe"):
        folded = folded[:-4]
    return folded


def _text(value: object, where: str) -> str:
    if not isinstance(value, str) or not value or len(value) > MAX_STRING:
        raise HostTableError(f"{where}: expected a non-empty string of at most {MAX_STRING} characters")
    return value


def parse_table(raw: bytes) -> dict[tuple[str, str], dict[str, str]]:
    """Validate table bytes and return {(platform, normalised name): host record}."""
    if len(raw) > MAX_TABLE_BYTES:
        raise HostTableError(f"table exceeds {MAX_TABLE_BYTES} bytes")
    try:
        doc = json.loads(raw.decode("utf-8"))
    except (UnicodeDecodeError, ValueError) as exc:
        raise HostTableError(f"table is not valid UTF-8 JSON: {exc}") from exc
    if not isinstance(doc, dict):
        raise HostTableError("table root must be an object")
    if doc.get("schema_version") != SCHEMA_VERSION or isinstance(doc.get("schema_version"), bool):
        raise HostTableError(f"unsupported schema_version, expected {SCHEMA_VERSION}")
    hosts = doc.get("hosts")
    if not isinstance(hosts, list) or len(hosts) > MAX_HOSTS:
        raise HostTableError("hosts must be a list within bounds")

    index: dict[tuple[str, str], dict[str, str]] = {}
    host_ids: set[str] = set()
    for i, host in enumerate(hosts):
        where = f"hosts[{i}]"
        if not isinstance(host, dict):
            raise HostTableError(f"{where}: must be an object")
        host_id = _text(host.get("host_id"), f"{where}.host_id")
        if host_id in host_ids:
            raise HostTableError(f"{where}: duplicate host_id {host_id!r}")
        host_ids.add(host_id)
        display = _text(host.get("display_name"), f"{where}.display_name")
        source = _text(host.get("source_url"), f"{where}.source_url")
        if not source.startswith("https://"):
            raise HostTableError(f"{where}.source_url must be https")
        matches = host.get("match")
        if not isinstance(matches, list) or not matches or len(matches) > MAX_MATCHES_PER_HOST:
            raise HostTableError(f"{where}.match must be a non-empty list within bounds")
        for j, m in enumerate(matches):
            mwhere = f"{where}.match[{j}]"
            if not isinstance(m, dict):
                raise HostTableError(f"{mwhere}: must be an object")
            platform = m.get("platform")
            if platform not in PLATFORMS:
                raise HostTableError(f"{mwhere}.platform must be one of {PLATFORMS}")
            exe = _text(m.get("executable_name"), f"{mwhere}.executable_name")
            if any(c in exe for c in "/\\") or exe.casefold().endswith((".exe", ".app")):
                raise HostTableError(f"{mwhere}.executable_name must be a bare name without path, .exe or .app")
            key = (platform, normalise_executable_name(exe))
            if key in index:
                raise HostTableError(f"{mwhere}: {key} is already claimed by {index[key]['host_id']!r}")
            index[key] = {"host_id": host_id, "display_name": display, "source_url": source}
    return index


def load_table(path: Path | None = None) -> dict[tuple[str, str], dict[str, str]]:
    target = path or DEFAULT_TABLE_PATH
    try:
        with open(target, "rb") as fh:
            raw = fh.read(MAX_TABLE_BYTES + 1)
    except OSError as exc:
        raise HostTableError(f"cannot read host table {target}: {exc.strerror}") from exc
    return parse_table(raw)


_cache: dict[tuple[str, str], dict[str, str]] | None = None


def identify_host(
    executable_name: str | None,
    jucehost_recognised: bool,
    jucehost_name: str | None,
    platform: str | None = None,
    table: dict[tuple[str, str], dict[str, str]] | None = None,
) -> HostIdentity:
    """Name the host. A wrapper-recognised host is returned unchanged.

    platform is 'windows', 'macos' or 'linux'; None matches any platform, and
    is rejected as ambiguous if two platforms map the name to different hosts.
    """
    global _cache
    if jucehost_recognised and jucehost_name:
        return HostIdentity(True, jucehost_name, IDENT_JUCE, "directly_observed")
    unmatched = HostIdentity(False, None, IDENT_NONE, "unknown_unobserved")
    if not isinstance(executable_name, str) or not executable_name:
        return unmatched
    if platform is not None and platform not in PLATFORMS:
        return unmatched
    if table is None:
        if _cache is None:
            _cache = load_table()
        table = _cache
    name = normalise_executable_name(executable_name)
    hits = {
        rec["host_id"]: rec
        for (plat, exe), rec in table.items()
        if exe == name and (platform is None or plat == platform)
    }
    if len(hits) != 1:
        return unmatched
    rec = next(iter(hits.values()))
    return HostIdentity(
        True, rec["display_name"], IDENT_INFERRED, "inferred",
        rec["host_id"], rec["display_name"], rec["source_url"],
    )
