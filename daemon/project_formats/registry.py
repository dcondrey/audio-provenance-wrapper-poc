"""Project-format registry: maps a project path to a parser or an explicit refusal.

A format is either `supported` (has a parser returning a ProjectSnapshot) or
`unsupported` (recognised by extension only). Unsupported formats never yield
structure: callers receive UnsupportedProjectFormat and report it as such.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from pathlib import Path
from typing import TYPE_CHECKING, Callable

if TYPE_CHECKING:
    from daemon.project_differ.differ import ProjectSnapshot

SUPPORTED = "supported"
UNSUPPORTED = "unsupported"

# How far a supported parser has been checked. Consumers must not read more
# confidence into a snapshot than this states.
REAL_FILES = "real_files"
CONSTRUCTED_ONLY = "constructed_fixtures_only"

# Untrusted-input ceiling shared by text/binary parsers registered here.
MAX_PROJECT_FILE_BYTES = 64 * 1024 * 1024


class UnsupportedProjectFormat(Exception):
    """The path is a recognised project type for which no parser exists."""

    def __init__(self, project_format: "ProjectFormat", path: Path) -> None:
        self.project_format = project_format
        self.path = path
        super().__init__(
            f"{project_format.host} projects ({project_format.format_id}) are not supported: "
            f"{project_format.reason}"
        )


@dataclass(frozen=True)
class ProjectFormat:
    format_id: str
    host: str
    extensions: tuple[str, ...]
    status: str
    reason: str = ""
    parser: Callable[[Path], "ProjectSnapshot"] | None = None
    # REAL_FILES or CONSTRUCTED_ONLY for supported formats; empty for unsupported ones.
    validation: str = ""

    @property
    def supported(self) -> bool:
        return self.status == SUPPORTED and self.parser is not None

    def parse(self, path: Path) -> "ProjectSnapshot":
        if not self.supported:
            raise UnsupportedProjectFormat(self, path)
        return self.parser(path)  # type: ignore[misc]


_BY_EXTENSION: dict[str, ProjectFormat] = {}
_FORMATS: list[ProjectFormat] = []


def register(project_format: ProjectFormat) -> ProjectFormat:
    for extension in project_format.extensions:
        if extension != extension.lower() or not extension.startswith("."):
            raise ValueError(f"extension must be lowercase and dotted: {extension!r}")
        if extension in _BY_EXTENSION:
            raise ValueError(f"{extension} is already registered to {_BY_EXTENSION[extension].format_id}")
        _BY_EXTENSION[extension] = project_format
    _FORMATS.append(project_format)
    return project_format


def registered_formats() -> tuple[ProjectFormat, ...]:
    return tuple(_FORMATS)


def detect_format(path: Path) -> ProjectFormat | None:
    """Look up by extension only; never reads the file. None when unrecognised."""
    return _BY_EXTENSION.get(path.suffix.lower())


def parse_project(path: Path) -> "ProjectSnapshot":
    """Parse `path` with its registered parser.

    An unrecognised extension keeps the historical behaviour of attempting the
    Ableton .als parser, whose gzip/XML checks reject anything else.
    """
    project_format = detect_format(path) or _BY_EXTENSION[".als"]
    return project_format.parse(path)


def unsupported_format_event(project_format: ProjectFormat, path: Path) -> dict[str, object]:
    """Evidence record stating that a watched project could not be structurally observed."""
    return {
        "event_type": "project_format_unsupported",
        "proof_level": "unknown_unobserved",
        "project_format": project_format.format_id,
        "host": project_format.host,
        "file_extension": path.suffix.lower(),
        "reason": project_format.reason,
        "timestamp_ms": int(time.time() * 1000),
        "daemon_observed_monotonic_ms": int(time.monotonic_ns() // 1_000_000),
    }
