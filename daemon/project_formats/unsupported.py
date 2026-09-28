"""Hosts recognised by extension only.

Nothing here reads or interprets file contents. Each entry states why, so a
report never implies structure that was not extracted. Formats are supported
only when a documented layout is parseable with the standard library and has
been checked against a real or documented sample.
"""

from __future__ import annotations

from .registry import UNSUPPORTED, ProjectFormat, register

_PROPRIETARY = "proprietary binary format with no public specification; no parser is provided"

_ENTRIES = (
    ProjectFormat(
        "logic_pro", "Logic Pro", (".logicx", ".logic"), UNSUPPORTED,
        "package/bundle with proprietary internal project data; no parser is provided",
    ),
    ProjectFormat("cubase", "Cubase/Nuendo", (".cpr", ".npr"), UNSUPPORTED, _PROPRIETARY),
    ProjectFormat("fl_studio", "FL Studio", (".flp",), UNSUPPORTED, _PROPRIETARY),
    ProjectFormat("pro_tools", "Pro Tools", (".ptx", ".ptf"), UNSUPPORTED, _PROPRIETARY),
    ProjectFormat(
        "bitwig", "Bitwig Studio", (".bwproject",), UNSUPPORTED,
        "internal layout not verified against a real or documented sample; no parser is provided",
    ),
    ProjectFormat(
        "studio_one", "Studio One", (".song",), UNSUPPORTED,
        "internal layout not verified against a real or documented sample; no parser is provided",
    ),
    ProjectFormat("cakewalk", "Cakewalk", (".cwp",), UNSUPPORTED, _PROPRIETARY),
    ProjectFormat(
        "garageband", "GarageBand", (".band",), UNSUPPORTED,
        "package/bundle with proprietary internal project data; no parser is provided",
    ),
)

for _entry in _ENTRIES:
    register(_entry)
