"""Summarise a train_log.jsonl loss curve.

The message terms are reported over MARKED STEPS ONLY. With `unmarked_fraction: 0.5` a small batch
draws an all-unmarked batch some of the time; `compute_losses` then reports `message = 0` and
`bit_accuracy = NaN` for that step, which are not measurements and must not be averaged in.
"""

import argparse
import json
import math
from pathlib import Path

FIELDS = ("total", "message", "message_per_frame", "detection", "perceptual", "spectral",
          "q_coexistence", "bit_accuracy")


def blocks(rows: list[dict], count: int) -> list[tuple[int, int, list[dict]]]:
    size = max(len(rows) // count, 1)
    return [(index * size, min((index + 1) * size, len(rows)), rows[index * size : (index + 1) * size])
            for index in range(count)]


def mean(rows: list[dict], field: str) -> float | None:
    values = [row[field] for row in rows
              if field in row and row[field] is not None and not math.isnan(row[field])]
    return sum(values) / len(values) if values else None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("log")
    parser.add_argument("--blocks", type=int, default=6)
    args = parser.parse_args()

    rows = [json.loads(line) for line in Path(args.log).read_text(encoding="utf-8").splitlines()]
    steps = [row for row in rows if row.get("event") == "step"]
    marked = [row for row in steps if not math.isnan(row.get("bit_accuracy", float("nan")))]
    print(f"{len(steps)} logged steps, {len(marked)} with at least one marked row in the batch "
          f"({len(steps) - len(marked)} all-unmarked batches carry no message measurement)")

    header = f"{'steps':>13s} {'phase':>5s} " + " ".join(f"{field:>13s}" for field in FIELDS)
    print(header)
    for low, high, block in blocks(steps, args.blocks):
        block_marked = [row for row in block if not math.isnan(row.get("bit_accuracy", float("nan")))]
        cells = []
        for field in FIELDS:
            source = block_marked if field in ("message", "message_per_frame", "bit_accuracy") else block
            value = mean(source, field)
            cells.append("          n/a" if value is None else f"{value:13.4f}")
        phases = sorted({row["phase"] for row in block})
        print(f"{low:6d}-{high - 1:<6d} {'/'.join(phases):>5s} " + " ".join(cells))

    # `seconds_per_step` in the log is cumulative, so elapsed[i] = seconds_per_step[i] * (i + 1) and
    # the per-step cost is its first difference. The last quarter is the number worth quoting for
    # kill criterion K8: the opening steps carry import, first-touch allocation and any other job
    # that happened to be sharing the machine.
    elapsed = [row["seconds_per_step"] * (row["step"] + 1) for row in steps]
    if len(elapsed) > 8:
        tail = len(elapsed) // 4
        steady = (elapsed[-1] - elapsed[-1 - tail]) / tail
        print(f"\nsteady-state {steady:.2f} s/step over the last {tail} steps "
              f"-> {steady * 320_000 / 86_400:.1f} days projected for 320k steps")

    finished = next((row for row in rows if row.get("event") == "finished"), None)
    if finished:
        print(f"\nwall {finished['wall_seconds']:.1f} s, {finished['seconds_per_step']:.2f} s/step, "
              f"projected {finished['projected_320k_days']:.1f} days for a 320k-step run "
              f"(K8 kills above 14), codec calls {finished['codec_calls']}, "
              f"failures {finished['codec_failures']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
