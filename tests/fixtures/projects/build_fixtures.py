"""Build the container fixtures and regenerate every golden snapshot.

Run from the repository root:  ./.venv/bin/python tests/fixtures/projects/build_fixtures.py

Containers are written with a fixed timestamp so their bytes, and therefore the
file_hash recorded in each golden file, are reproducible.
"""

from __future__ import annotations

import struct
import sys
import zipfile
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from daemon.project_formats import parse_project  # noqa: E402
from daemon.project_formats._snapshot import golden_json  # noqa: E402
from module_builders import constructed_vcv  # noqa: E402

HERE = Path(__file__).resolve().parent
FIXED_TIME = (1980, 1, 1, 0, 0, 0)

FIXTURES = (
    "dawproject/basic.dawproject",
    "ardour/basic.ardour",
    "lmms/basic.mmp",
    "lmms/basic.mmpz",
    "puredata/A01.sinewave.pd",
    "maxpat/basic.maxpat",
    "milkytracker/test.xm",
    "milkytracker/test.mod",
    "vcv/basic.vcv",
)


def build_zip(target: Path, members: dict[str, bytes]) -> None:
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, data in members.items():
            info = zipfile.ZipInfo(name, FIXED_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, data)


def qcompress(data: bytes) -> bytes:
    """Qt's qCompress: 4-byte big-endian uncompressed length, then a zlib stream."""
    return struct.pack(">I", len(data)) + zlib.compress(data, 9)


def main() -> None:
    build_zip(HERE / "dawproject/basic.dawproject", {"project.xml": (HERE / "dawproject/project.xml").read_bytes()})
    (HERE / "vcv/basic.vcv").write_bytes(constructed_vcv())
    (HERE / "lmms/basic.mmpz").write_bytes(qcompress((HERE / "lmms/basic.mmp").read_bytes()))
    for relative in FIXTURES:
        path = HERE / relative
        (path.parent / (path.name + ".golden.json")).write_text(golden_json(parse_project(path)), encoding="utf-8")
        print("wrote", relative)


if __name__ == "__main__":
    main()
