"""Project-format registry. Importing this package registers every known format."""

from __future__ import annotations

from pathlib import Path

from . import unsupported as _unsupported  # noqa: F401  (registers on import)
from .registry import (
    CONSTRUCTED_ONLY,
    MAX_PROJECT_FILE_BYTES,
    REAL_FILES,
    SUPPORTED,
    UNSUPPORTED,
    ProjectFormat,
    UnsupportedProjectFormat,
    detect_format,
    parse_project,
    register,
    registered_formats,
    unsupported_format_event,
)


def _parse_als(path: Path):
    from daemon.project_differ.differ import extract_snapshot

    return extract_snapshot(path)


def _parse_reaper(path: Path):
    from .reaper import extract_reaper_snapshot

    return extract_reaper_snapshot(path)


def _parse_dawproject(path: Path):
    from .dawproject import extract_dawproject_snapshot

    return extract_dawproject_snapshot(path)


def _parse_ardour(path: Path):
    from .ardour import extract_ardour_snapshot

    return extract_ardour_snapshot(path)


def _parse_lmms(path: Path):
    from .lmms import extract_lmms_snapshot

    return extract_lmms_snapshot(path)


def _parse_pd(path: Path):
    from .puredata import extract_pd_snapshot

    return extract_pd_snapshot(path)


def _parse_maxpat(path: Path):
    from .maxpat import extract_maxpat_snapshot

    return extract_maxpat_snapshot(path)


def _parse_module(path: Path):
    from .tracker import extract_module_snapshot

    return extract_module_snapshot(path)


def _parse_vcv(path: Path):
    from .vcv import extract_vcv_snapshot

    return extract_vcv_snapshot(path)


register(
    ProjectFormat(
        "ableton_als", "Ableton Live", (".als",), SUPPORTED,
        "gzip XML; tests use synthetic documents, no real Live-saved file is committed",
        parser=_parse_als, validation=CONSTRUCTED_ONLY,
    )
)
register(
    ProjectFormat(
        "reaper_rpp",
        "REAPER",
        (".rpp",),
        SUPPORTED,
        "plain-text RPP; layout is unofficial and checked only against hand-written fixtures",
        parser=_parse_reaper,
        validation=CONSTRUCTED_ONLY,
    )
)
register(
    ProjectFormat(
        "dawproject",
        "DAWproject (Bitwig, Cubase, Studio One and others export it)",
        (".dawproject",),
        SUPPORTED,
        "open zip of XML per Project.xsd; the only fixture is a Bitwig Studio 5.0 project.xml from the spec README, zipped here, not a real .dawproject file",
        parser=_parse_dawproject,
        validation=CONSTRUCTED_ONLY,
    )
)
register(
    ProjectFormat(
        "ardour",
        "Ardour",
        (".ardour",),
        SUPPORTED,
        "XML layout taken from Ardour's source; checked only against fixtures constructed from that source",
        parser=_parse_ardour,
        validation=CONSTRUCTED_ONLY,
    )
)
register(
    ProjectFormat(
        "lmms",
        "LMMS",
        (".mmp", ".mmpz"),
        SUPPORTED,
        "XML or qCompress-ed XML per LMMS source; committed fixtures are constructed, one real demo was checked locally",
        parser=_parse_lmms,
        validation=CONSTRUCTED_ONLY,
    )
)
register(
    ProjectFormat(
        "pure_data",
        "Pure Data",
        (".pd",),
        SUPPORTED,
        "plain-text patch per Pd source; no tempo or clips, one track per canvas and one device per object box",
        parser=_parse_pd,
        validation=REAL_FILES,
    )
)
register(
    ProjectFormat(
        "max_patcher",
        "Max",
        (".maxpat",),
        SUPPORTED,
        "JSON patcher with no published spec; keys observed in one Max 7 file, fixture is constructed",
        parser=_parse_maxpat,
        validation=CONSTRUCTED_ONLY,
    )
)
register(
    ProjectFormat(
        "milkytracker",
        "MilkyTracker / module trackers",
        (".xm", ".mod"),
        SUPPORTED,
        "FastTracker 2 XM 1.04 and 31-sample ProTracker MOD per MilkyTracker's loaders: header, instruments, sample names and lengths, pattern hash; no audio is decoded; fixtures are OpenMPT's own test modules",
        parser=_parse_module,
        validation=REAL_FILES,
    )
)
register(
    ProjectFormat(
        "vcv_rack",
        "VCV Rack",
        (".vcv",),
        SUPPORTED,
        "Rack 2 tar+Zstandard or legacy JSON patch per Rack v2.6.6 src/patch.cpp; module list, cable count, param and data presence; the committed fixture is constructed, a real Rack 2.6.6 template patch was checked locally",
        parser=_parse_vcv,
        validation=CONSTRUCTED_ONLY,
    )
)

__all__ = [
    "CONSTRUCTED_ONLY",
    "REAL_FILES",
    "MAX_PROJECT_FILE_BYTES",
    "SUPPORTED",
    "UNSUPPORTED",
    "ProjectFormat",
    "UnsupportedProjectFormat",
    "detect_format",
    "parse_project",
    "register",
    "registered_formats",
    "unsupported_format_event",
]
