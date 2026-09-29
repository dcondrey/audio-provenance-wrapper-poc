"""Max patcher (.maxpat) parser.

A .maxpat is JSON: `{"patcher": {"boxes": [{"box": {...}}], "lines":
[{"patchline": {"source": [id, outlet], "destination": [id, inlet]}}], ...}}`,
with subpatchers nested as a box's own `"patcher"`. Cycling '74 publishes no
formal specification (https://cycling74.com/forums/specification-for-maxpat-json-format
is a community thread); the keys used here are the ones observed in a patcher
saved by Max 7 and shipped in the MIT-licensed Cycling74/max-sdk
(help/dummy.maxhelp at 15b6fe17). Nothing else about the format is assumed, and
unknown keys are ignored.

Mapping onto the shared snapshot matches Pure Data: one track per patcher (root
then nested subpatchers in document order), each box a device (`newobj` text, or
`maxclass` with text when present; comments excluded), and any text atom ending
in an audio file extension a sample reference. Geometry keys are excluded from
the device-chain hash; patchlines are included.

The input is untrusted: size, JSON depth and node count are capped and nothing
is evaluated.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any

from . import _safe
from ._snapshot import NeutralProject, NeutralTrack, build_snapshot, digest_of

MAX_PATCHERS = 100_000

_AUDIO_EXTENSIONS = (".wav", ".aif", ".aiff", ".flac", ".ogg", ".mp3", ".w64", ".caf")
_LAYOUT_KEYS = frozenset(
    {"patching_rect", "presentation_rect", "rect", "presentation", "fontsize", "fontname", "fontface", "bgcolor"}
)


def _strip(value: Any) -> Any:
    """Drop layout keys and nested patchers (they are their own tracks); bounded by the JSON depth cap."""
    if isinstance(value, dict):
        return {k: _strip(v) for k, v in sorted(value.items()) if k not in _LAYOUT_KEYS and k != "patcher"}
    if isinstance(value, list):
        return [_strip(v) for v in value]
    return value


def _box_label(box: dict[str, Any]) -> str | None:
    maxclass = box.get("maxclass")
    text = box.get("text")
    text = text if isinstance(text, str) else ""
    if maxclass == "comment":
        return None
    if maxclass == "newobj":
        return text
    return f"{maxclass}: {text}" if text else str(maxclass)


def extract_maxpat(data: bytes) -> NeutralProject:
    document = _safe.parse_json(data)
    root = document.get("patcher") if isinstance(document, dict) else None
    if not isinstance(root, dict):
        raise ValueError("not a Max patcher: no top-level 'patcher' object")

    tracks: list[NeutralTrack] = []
    # (patcher, name, kind, parent id); explicit stack keeps document order via reversed pushes.
    pending: list[tuple[dict[str, Any], str, str, str]] = [(root, "main", "Patch", "")]
    while pending:
        patcher, name, kind, parent = pending.pop()
        if len(tracks) >= MAX_PATCHERS:
            raise ValueError(f"Max patch has more than {MAX_PATCHERS} patchers; refusing to parse")
        track_id = f"patcher-{len(tracks)}"
        devices: list[str] = []
        samples: list[str] = []
        stripped_boxes: list[Any] = []
        children: list[tuple[dict[str, Any], str, str, str]] = []
        for entry in patcher.get("boxes", []) if isinstance(patcher.get("boxes"), list) else []:
            box = entry.get("box") if isinstance(entry, dict) else None
            if not isinstance(box, dict):
                continue
            stripped_boxes.append(_strip(box))
            label = _box_label(box)
            if label is not None:
                devices.append(label)
                samples.extend(a for a in label.split() if a.lower().endswith(_AUDIO_EXTENSIONS))
            if isinstance(box.get("patcher"), dict):
                text = box.get("text") if isinstance(box.get("text"), str) else ""
                children.append((box["patcher"], text or str(box.get("id", "")), "Subpatch", track_id))
        lines = patcher.get("lines", []) if isinstance(patcher.get("lines"), list) else []
        tracks.append(
            NeutralTrack(
                track_id=track_id,
                name=name,
                track_type=kind,
                devices=tuple(devices),
                device_chain_hashes=frozenset({digest_of(stripped_boxes, _strip(lines))}),
                extra_sample_refs=tuple(dict.fromkeys(samples)),
                group_id=parent,
            )
        )
        pending.extend(reversed(children))
    return NeutralProject(project_format="max_patcher", tracks=tracks)


def extract_maxpat_snapshot(path: Path):
    return build_snapshot(extract_maxpat(_safe.read_project_bytes(path)), path)
