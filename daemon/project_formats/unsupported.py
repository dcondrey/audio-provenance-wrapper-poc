"""Hosts recognised by extension only.

Nothing here reads or interprets file contents. Each entry states why, so a
report never implies structure that was not extracted. Formats are supported
only when a documented layout is parseable with the standard library and has
been checked against a real or documented sample. Where the host can export the
open DAWproject interchange format, the reason says so.
"""

from __future__ import annotations

from .registry import UNSUPPORTED, ProjectFormat, register

_PROPRIETARY = "proprietary binary format with no public specification; no parser is provided"
_PACKAGE = "package/bundle with proprietary internal project data; no parser is provided"
_UNVERIFIED = "internal layout not verified against a real or documented sample; no parser is provided"
_DAWPROJECT = "; the host can export open .dawproject, which is parsed"


def _entry(format_id: str, host: str, extensions: tuple[str, ...], reason: str) -> ProjectFormat:
    return ProjectFormat(format_id, host, extensions, UNSUPPORTED, reason)


_ENTRIES = (
    _entry("logic_pro", "Logic Pro", (".logicx", ".logic"), _PACKAGE),
    _entry("cubase", "Cubase/Nuendo", (".cpr", ".npr"), _PROPRIETARY + _DAWPROJECT),
    _entry("fl_studio", "FL Studio", (".flp",), _PROPRIETARY),
    _entry("pro_tools", "Pro Tools", (".ptx", ".ptf"), _PROPRIETARY),
    _entry("bitwig", "Bitwig Studio", (".bwproject",), _UNVERIFIED + _DAWPROJECT),
    _entry("studio_one", "Studio One / Fender Studio Pro", (".song",), _UNVERIFIED + _DAWPROJECT),
    _entry("cakewalk", "Cakewalk", (".cwp",), _PROPRIETARY),
    _entry("garageband", "GarageBand", (".band",), _PACKAGE),
    _entry("reason", "Reason", (".reason",), _PROPRIETARY),
    _entry("sibelius", "Sibelius", (".sib",), _PROPRIETARY),
    _entry("dorico", "Dorico", (".dorico",), "zip container whose internal XML has no public specification; no parser is provided"),
    _entry("finale", "Finale", (".musx", ".mus"), _PROPRIETARY + "; MusicXML export is the interchange route and is not parsed here"),
    _entry("vegas", "VEGAS Pro", (".veg",), _PROPRIETARY),
    _entry("acid", "ACID Pro", (".acd",), _PROPRIETARY),
    _entry("premiere", "Premiere Pro", (".prproj",), "compressed XML with no public specification; no parser is provided"),
    _entry("audition", "Audition", (".sesx",), "XML with no published schema; no parser is provided"),
    _entry("resolve", "DaVinci Resolve", (".drp",), _PROPRIETARY),
    _entry("audiomulch", "AudioMulch", (".amh",), "XML that its developer states is undocumented and may change without notice; no parser is provided"),
    _entry("reaktor", "Reaktor", (".ens", ".rkplr"), _PROPRIETARY),
    _entry(
        "renoise", "Renoise", (".xrns",),
        "zip of Song.xml, but Renoise publishes no schema or format specification (the xrnx repository, github.com/renoise/xrnx, has no XSD and covers tool scripting only), so no primary source grounds extraction; no parser is provided",
    ),
    _entry(
        "audacity", "Audacity", (".aup3",),
        "SQLite database whose schema Audacity does not publish; no parser is provided",
    ),
    _entry(
        "tracktion_waveform", "Tracktion Waveform", (".tracktionedit",),
        "believed to be a JUCE ValueTree serialised as XML, but the layout is not verified at primary level; no parser is provided",
    ),
)

for _entry_ in _ENTRIES:
    register(_entry_)
