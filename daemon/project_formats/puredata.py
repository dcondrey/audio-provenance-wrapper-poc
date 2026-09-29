"""Pure Data (.pd) patch parser.

Pd patches are plain text: a sequence of `;`-terminated messages whose atoms are
whitespace separated, with `\\` escaping `;`, `,`, `$` and `\\` (Pd source,
src/g_readwrite.c and the binbuf parser; BSD-licensed, https://github.com/pure-data/pure-data).
Records used here:

  #N canvas x y w h font;            root canvas
  #N canvas x y w h name vis;        subpatch or graph
  #X obj|msg|floatatom|symbolatom|text x y ...;
  #X restore x y pd name;            closes the innermost canvas
  #X connect src outlet dst inlet;

Pd has no tempo, clips or plug-in chain, so the mapping onto the shared
snapshot is: one track per canvas (root, subpatches, graphs), each `#X obj`
box a device (`class args`), and any atom ending in an audio file extension a
sample reference. Box coordinates are excluded from the device-chain hash so
that moving boxes is not reported as an edit; connections are included.

The input is untrusted: size, statement count and canvas nesting are capped and
nothing is evaluated.
"""

from __future__ import annotations

from pathlib import Path

from . import _safe
from ._snapshot import NeutralProject, NeutralTrack, build_snapshot, digest_of

MAX_STATEMENTS = 1_000_000
MAX_CANVAS_DEPTH = 64
MAX_CANVASES = 100_000

_AUDIO_EXTENSIONS = (".wav", ".aif", ".aiff", ".flac", ".ogg", ".mp3", ".w64", ".caf")
_BOX_TYPES = frozenset({"obj", "msg", "floatatom", "symbolatom", "text"})


def tokenize(text: str) -> list[list[str]]:
    """Split Pd text into statements of atoms. Raises ValueError past MAX_STATEMENTS."""
    statements: list[list[str]] = []
    atoms: list[str] = []
    atom: list[str] = []
    in_atom = False

    def flush_atom() -> None:
        nonlocal in_atom
        if in_atom:
            atoms.append("".join(atom))
            atom.clear()
            in_atom = False

    index, length = 0, len(text)
    while index < length:
        char = text[index]
        if char == "\\" and index + 1 < length:
            atom.append(text[index + 1])
            in_atom = True
            index += 2
            continue
        if char == ";":
            flush_atom()
            statements.append(atoms.copy())
            atoms.clear()
            if len(statements) > MAX_STATEMENTS:
                raise ValueError(f"Pd patch has more than {MAX_STATEMENTS} statements; refusing to parse")
        elif char == ",":
            flush_atom()
            atoms.append(",")
        elif char.isspace():
            flush_atom()
        else:
            atom.append(char)
            in_atom = True
        index += 1
    flush_atom()
    if atoms:
        statements.append(atoms.copy())
    return [s for s in statements if s]


class _Canvas:
    def __init__(self, canvas_id: str, name: str, kind: str, group: str) -> None:
        self.canvas_id = canvas_id
        self.name = name
        self.kind = kind
        self.group = group
        self.devices: list[str] = []
        self.samples: list[str] = []
        self.normalised: list[list[str]] = []


def extract_pd(text: str) -> NeutralProject:
    statements = tokenize(text)
    if not statements or statements[0][:2] != ["#N", "canvas"]:
        raise ValueError("not a Pd patch: first record is not '#N canvas'")

    canvases: list[_Canvas] = []
    stack: list[_Canvas] = []
    for record in statements:
        head = record[0]
        kind = record[1] if len(record) > 1 else ""
        if head == "#N" and kind == "canvas":
            if len(stack) >= MAX_CANVAS_DEPTH:
                raise ValueError(f"Pd canvas nesting deeper than {MAX_CANVAS_DEPTH}; refusing to parse")
            if len(canvases) >= MAX_CANVASES:
                raise ValueError(f"Pd patch has more than {MAX_CANVASES} canvases; refusing to parse")
            if not stack:
                if canvases:
                    raise ValueError("Pd patch has a second top-level canvas")
                canvas = _Canvas("canvas-0", "main", "Patch", "")
            else:
                name = record[6] if len(record) > 6 else ""
                canvas = _Canvas(
                    f"canvas-{len(canvases)}", name, "Graph" if name == "(subpatch)" else "Subpatch", stack[-1].canvas_id
                )
            canvases.append(canvas)
            stack.append(canvas)
            continue
        if not stack:
            raise ValueError("Pd record outside the root canvas")
        current = stack[-1]
        if head == "#X" and kind == "restore":
            if len(stack) == 1:
                raise ValueError("Pd 'restore' without an open subpatch")
            stack.pop()
            stack[-1].normalised.append(["restore", *record[4:]])
        elif head == "#X":
            # Box coordinates (records 2 and 3) are layout, not content.
            current.normalised.append([kind, *record[4:]] if kind in _BOX_TYPES else record[1:])
            if kind == "obj":
                current.devices.append(" ".join(record[4:]))
            if kind in ("obj", "msg"):
                current.samples.extend(a for a in record[4:] if a.lower().endswith(_AUDIO_EXTENSIONS))
        else:
            current.normalised.append(record)  # `#A` array data and other records
    return NeutralProject(
        project_format="pure_data",
        tracks=[
            NeutralTrack(
                track_id=c.canvas_id,
                name=c.name,
                track_type=c.kind,
                devices=tuple(c.devices),
                device_chain_hashes=frozenset({digest_of(c.normalised)}),
                extra_sample_refs=tuple(dict.fromkeys(c.samples)),
                group_id=c.group,
            )
            for c in canvases
        ],
    )


def extract_pd_snapshot(path: Path):
    text = _safe.read_project_bytes(path).decode("utf-8", errors="replace")
    return build_snapshot(extract_pd(text), path)
