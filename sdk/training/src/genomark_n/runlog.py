"""Run provenance, logging and checkpointing.

A checkpoint that does not carry the config, the seed report and the git revision that produced it
cannot support the model card's training-provenance section, and a model card whose provenance was
reconstructed from memory is exactly the artifact spec 12.4 forbids.
"""

import hashlib
import json
import platform
import subprocess
import time
from dataclasses import asdict, dataclass
from pathlib import Path

import torch

from .config import Config
from .seeding import SeedReport


def git_revision(root: Path) -> str | None:
    try:
        finished = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True, timeout=10, check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if finished.returncode != 0:
        return None
    return finished.stdout.decode().strip() or None


def digest_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def digest_file(path: str | Path) -> str:
    hasher = hashlib.sha256()
    with Path(path).open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            hasher.update(block)
    return hasher.hexdigest()


@dataclass
class RunRecord:
    started_at: str
    config_digest: str
    git_revision: str | None
    torch_version: str
    platform: str
    device: str
    seed_note: str

    def to_dict(self) -> dict:
        return asdict(self)


class RunDirectory:
    def __init__(self, root: str | Path, config: Config, seed: SeedReport, repo_root: Path) -> None:
        self.root = Path(root)
        (self.root / "checkpoints").mkdir(parents=True, exist_ok=True)
        self.config = config
        self.log_path = self.root / "train_log.jsonl"
        self.record = RunRecord(
            started_at=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            config_digest=digest_text(config.to_json()),
            git_revision=git_revision(repo_root),
            torch_version=torch.__version__,
            platform=f"{platform.system()} {platform.machine()} python {platform.python_version()}",
            device=seed.device,
            seed_note=seed.note,
        )
        (self.root / "config.json").write_text(config.to_json(), encoding="utf-8")
        (self.root / "run.json").write_text(
            json.dumps({**self.record.to_dict(), "seed": asdict(seed)}, indent=2), encoding="utf-8"
        )

    def log(self, payload: dict) -> None:
        with self.log_path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(payload) + "\n")

    def checkpoint(self, step: int, model: torch.nn.Module, optimiser, ema: dict | None) -> Path:
        path = self.root / "checkpoints" / f"step_{step:08d}.pt"
        torch.save(
            {
                "step": step,
                "model": model.state_dict(),
                "optimiser": optimiser.state_dict(),
                "ema": ema,
                "config": self.config.to_dict(),
                "run": self.record.to_dict(),
            },
            path,
        )
        latest = self.root / "checkpoints" / "latest.pt"
        if latest.exists():
            latest.unlink()
        latest.symlink_to(path.name)
        return path


class Ema:
    """Spec 9.5: EMA of the encoder weights with decay 0.9999 for the shipped checkpoint."""

    def __init__(self, module: torch.nn.Module, decay: float) -> None:
        self.decay = decay
        self.shadow = {name: value.detach().clone() for name, value in module.state_dict().items()}

    @torch.no_grad()
    def update(self, module: torch.nn.Module) -> None:
        for name, value in module.state_dict().items():
            stored = self.shadow[name]
            if not stored.is_floating_point():
                stored.copy_(value)
                continue
            stored.mul_(self.decay).add_(value.detach(), alpha=1.0 - self.decay)

    def state_dict(self) -> dict:
        return {name: value.clone() for name, value in self.shadow.items()}
