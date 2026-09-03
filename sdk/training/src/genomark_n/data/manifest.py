"""JSONL corpus manifests. One line per item, licence declared per item, gated on load."""

import json
from dataclasses import asdict, dataclass
from pathlib import Path

from .licences import require_allowed_licence, require_allowed_source


@dataclass(frozen=True)
class CorpusEntry:
    path: str
    licence: str
    source: str
    attribution: str | None = None
    url: str | None = None
    sample_rate: int | None = None
    seconds: float | None = None

    def resolve(self, root: Path) -> Path:
        candidate = Path(self.path)
        return candidate if candidate.is_absolute() else root / candidate


def load_manifest(path: str | Path, require_files: bool = True) -> list[CorpusEntry]:
    manifest_path = Path(path)
    root = manifest_path.parent
    entries: list[CorpusEntry] = []
    with manifest_path.open(encoding="utf-8") as handle:
        for number, line in enumerate(handle, start=1):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            record = json.loads(line)
            where = f"{manifest_path}:{number}"
            if "licence" not in record:
                raise KeyError(f"{where}: manifest entry has no 'licence' field")
            require_allowed_source(record.get("source", "unknown"), where)
            require_allowed_licence(record["licence"], where)
            entry = CorpusEntry(**record)
            if require_files and not entry.resolve(root).exists():
                raise FileNotFoundError(f"{where}: {entry.resolve(root)} does not exist")
            entries.append(entry)
    if not entries:
        raise ValueError(f"{manifest_path}: no usable entries after licence filtering")
    return entries


def write_manifest(path: str | Path, entries: list[CorpusEntry]) -> None:
    with Path(path).open("w", encoding="utf-8") as handle:
        for entry in entries:
            handle.write(json.dumps({k: v for k, v in asdict(entry).items() if v is not None}) + "\n")
