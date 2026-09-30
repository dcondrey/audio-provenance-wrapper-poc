"""VCV Rack patch (.vcv) parser.

Grounding: VCV Rack v2.6.6 (tag object 061ccf63c1758599396ac1bb10d47345d9d34076,
GPL-3.0-or-later, https://github.com/VCVRack/Rack/tree/v2.6.6). Nothing from Rack
is copied.

  src/patch.cpp   Manager::load / isPatchLegacyV1: a Rack 2 patch is a tar
                  archive compressed with Zstandard (libarchive `pax_restricted`
                  + zstd filter, src/system.cpp) holding `patch.json`; a file that
                  does not begin with the Zstandard magic 28 b5 2f fd is a
                  legacy (Rack 1) plain-JSON patch.
  Manager::toJson root keys: version, path, unsaved, zoom, gridOffset,
                  modules, cables, masterModuleId (plus `pos` per module added by
                  RackWidget::mergeJson).
  src/engine/Module.cpp Module::toJson: id, plugin, model, version, params
                  ([{id, value}]), bypass, leftModuleId, rightModuleId, data.
  src/engine/Cable.cpp Cable::toJson: id, outputModuleId, outputId,
                  inputModuleId, inputId. Engine::fromJson reads legacy `wires`
                  when `cables` is absent.

Mapping onto the shared snapshot: a `patch` track (container, Rack version and
counts), one `Module` track per module object (name `plugin/model`; devices state
version, parameter count, whether `data` is present and bypass; the device-chain
hash covers plugin, model, version, params, data and bypass but not id, position
or expander links), and one `Cables` track (a device per cable, hash over the
endpoint ids in order). Patches have no tempo, so `transport_bpm` is 0.0.

Module `data` is untrusted JSON: it is hashed and never interpreted. Other
archive members (module asset folders) are validated and never read. Non-finite
numbers are refused so Python and Rust hash identical values.
"""

from __future__ import annotations

import math
from pathlib import Path
from typing import Any

from . import _safe, _tarzst
from ._snapshot import NeutralProject, NeutralTrack, build_snapshot, digest_of

_INT64_MIN = -(2**63)
_INT64_MAX = 2**63 - 1


def _int_or_none(value: Any) -> int | None:
    if isinstance(value, int) and not isinstance(value, bool) and _INT64_MIN <= value <= _INT64_MAX:
        return value
    return None


def _ident(value: Any) -> str:
    number = _int_or_none(value)
    return "?" if number is None else str(number)


def _text(value: Any) -> str:
    return value if isinstance(value, str) else ""


def _check_numbers(value: Any) -> None:
    stack = [value]
    while stack:
        item = stack.pop()
        if isinstance(item, float) and not math.isfinite(item):
            raise ValueError("malformed JSON: non-finite number")
        if isinstance(item, dict):
            stack.extend(item.values())
        elif isinstance(item, list):
            stack.extend(item)


def extract_vcv(data: bytes) -> NeutralProject:
    container = "json (legacy)"
    raw = data
    if data[:4] == _tarzst.ZSTD_MAGIC:
        container = "tar+zstd"
        tar = _tarzst.inflate_zstd(data)
        matches = [(start, size) for name, start, size in _tarzst.read_tar(tar) if name == "patch.json"]
        if not matches:
            raise ValueError("VCV Rack archive has no patch.json")
        start, size = matches[0]
        raw = tar[start : start + size]
    # The archive is already capped at MAX_TAR_BYTES; a plain file at the project size cap.
    document = _safe.parse_json(raw, limit=_safe.MAX_TAR_BYTES)
    _check_numbers(document)
    if not isinstance(document, dict) or not isinstance(document.get("version"), str):
        raise ValueError("not a VCV Rack patch: no top-level 'version' string")

    modules = document.get("modules")
    module_objects = [m for m in modules if isinstance(m, dict)] if isinstance(modules, list) else []
    if "cables" in document:
        cables = document["cables"]
    else:
        cables = document.get("wires")
    cable_objects = [c for c in cables if isinstance(c, dict)] if isinstance(cables, list) else []

    tracks = [
        NeutralTrack(
            track_id="patch",
            name="patch",
            track_type="Patch",
            devices=(
                f"format: VCV Rack patch ({container})",
                f"rack version: {document['version']}",
                f"modules: {len(module_objects)}",
                f"cables: {len(cable_objects)}",
            ),
        )
    ]
    for position, module in enumerate(module_objects):
        plugin, model, version = _text(module.get("plugin")), _text(module.get("model")), _text(module.get("version"))
        params = module.get("params")
        bypass = module.get("bypass") if "bypass" in module else module.get("disabled")
        bypassed = bypass is True
        has_data = module.get("data") is not None
        devices = [f"{plugin}/{model}"]
        if version:
            devices.append(f"version: {version}")
        devices.append(f"params: {len(params) if isinstance(params, list) else 0}")
        if has_data:
            devices.append("data: present")
        if bypassed:
            devices.append("bypassed")
        module_id = _int_or_none(module.get("id"))
        tracks.append(
            NeutralTrack(
                track_id=f"module-{module_id}" if module_id is not None else f"module-pos-{position}",
                name=f"{plugin}/{model}",
                track_type="Module",
                devices=tuple(devices),
                device_chain_hashes=frozenset(
                    {
                        digest_of(
                            {
                                "plugin": plugin,
                                "model": model,
                                "version": version,
                                "params": params,
                                "data": module.get("data"),
                                "bypass": bypassed,
                            }
                        )
                    }
                ),
            )
        )
    endpoints = [
        [_int_or_none(c.get(k)) for k in ("outputModuleId", "outputId", "inputModuleId", "inputId")]
        for c in cable_objects
    ]
    tracks.append(
        NeutralTrack(
            track_id="cables",
            name="cables",
            track_type="Cables",
            devices=tuple(
                f"{_ident(c.get('outputModuleId'))}:{_ident(c.get('outputId'))} -> "
                f"{_ident(c.get('inputModuleId'))}:{_ident(c.get('inputId'))}"
                for c in cable_objects
            ),
            device_chain_hashes=frozenset({digest_of(endpoints)}),
        )
    )
    return NeutralProject(project_format="vcv_rack", tracks=tracks)


def extract_vcv_snapshot(path: Path):
    return build_snapshot(extract_vcv(_safe.read_project_bytes(path)), path)
