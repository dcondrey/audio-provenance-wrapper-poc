"""Project-format registry. Importing this package registers every known format."""

from __future__ import annotations

from pathlib import Path

from . import unsupported as _unsupported  # noqa: F401  (registers on import)
from .registry import (
    MAX_PROJECT_FILE_BYTES,
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


register(ProjectFormat("ableton_als", "Ableton Live", (".als",), SUPPORTED, parser=_parse_als))
register(
    ProjectFormat(
        "reaper_rpp",
        "REAPER",
        (".rpp",),
        SUPPORTED,
        "plain-text RPP; layout is unofficial and checked only against hand-written fixtures",
        parser=_parse_reaper,
    )
)

__all__ = [
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
