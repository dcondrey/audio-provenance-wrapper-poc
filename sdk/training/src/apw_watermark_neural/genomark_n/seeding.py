"""Reproducible seeding.

Full determinism is NOT claimed on MPS: several conv and pooling kernels there have no
deterministic implementation and torch.use_deterministic_algorithms raises rather than silently
degrading. CPU runs are deterministic; MPS runs are seeded but not bit-reproducible, and
`seed_everything` returns which of the two it achieved so a run record can say so.
"""

import os
import random
from dataclasses import dataclass

import numpy as np
import torch


@dataclass(frozen=True)
class SeedReport:
    seed: int
    device: str
    deterministic_algorithms: bool
    note: str


def resolve_device(requested: str) -> torch.device:
    if requested == "auto":
        if torch.cuda.is_available():
            return torch.device("cuda")
        if torch.backends.mps.is_available():
            return torch.device("mps")
        return torch.device("cpu")
    return torch.device(requested)


def seed_everything(seed: int, device: torch.device, deterministic: bool = True) -> SeedReport:
    os.environ["PYTHONHASHSEED"] = str(seed)
    random.seed(seed)
    np.random.seed(seed % (2**32))
    torch.manual_seed(seed)
    if torch.cuda.is_available():
        torch.cuda.manual_seed_all(seed)
    if device.type == "mps":
        torch.mps.manual_seed(seed)

    achieved = False
    note = "determinism not requested"
    if deterministic:
        if device.type == "cpu":
            torch.use_deterministic_algorithms(True)
            torch.backends.cudnn.deterministic = True
            torch.backends.cudnn.benchmark = False
            achieved = True
            note = "CPU deterministic algorithms enabled"
        else:
            note = (
                f"determinism requested but not available on {device.type}: seeded only, "
                "results are not bit-reproducible"
            )
    return SeedReport(seed=seed, device=str(device), deterministic_algorithms=achieved, note=note)


def example_seed(base_seed: int, epoch: int, index: int) -> int:
    """Per-example seed so augmentation is reproducible regardless of worker count or order."""
    mixed = (base_seed & 0xFFFF_FFFF) * 0x9E37_79B9
    mixed ^= (epoch & 0xFFFF_FFFF) * 0x85EB_CA6B
    mixed ^= (index & 0xFFFF_FFFF) * 0xC2B2_AE35
    mixed &= 0xFFFF_FFFF_FFFF_FFFF
    mixed ^= mixed >> 29
    mixed = (mixed * 0xBF58_476D_1CE4_E5B9) & 0xFFFF_FFFF_FFFF_FFFF
    mixed ^= mixed >> 32
    return mixed


def worker_init_fn(worker_id: int) -> None:
    base = int(torch.initial_seed()) & 0xFFFF_FFFF
    seed = (base + worker_id) & 0xFFFF_FFFF
    random.seed(seed)
    np.random.seed(seed)
