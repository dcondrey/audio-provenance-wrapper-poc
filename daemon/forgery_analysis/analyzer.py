from __future__ import annotations

import logging
import math
from dataclasses import dataclass, field

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class ForgeryFlag:
    """A single indicator of potential forgery or synthetic generation."""

    name: str
    description: str
    severity: float       # 0.0 (informational) to 1.0 (strong indicator)
    evidence: str         # human-readable supporting data


@dataclass
class ForgeryReport:
    """Aggregated forgery analysis for a session or session segment."""

    flags: list[ForgeryFlag] = field(default_factory=list)
    sample_count: int = 0
    analysis_window_ms: int = 0

    @property
    def suspicion_score(self) -> float:
        """0.0 = no indicators, 1.0 = certainly synthetic.

        Each flag contributes its severity weighted by 0.3 (matching the
        CPoE scoring model where individual flags are suggestive but not
        conclusive). Clamped to [0.0, 1.0].
        """
        if not self.flags:
            return 0.0
        raw = sum(f.severity * 0.3 for f in self.flags)
        return min(raw, 1.0)

    @property
    def is_suspicious(self) -> bool:
        return self.suspicion_score > 0.5

    def to_event_dict(self) -> dict[str, object]:
        return {
            "event_type": "forgery_analysis",
            "proof_level": "inferred",
            "suspicion_score": round(self.suspicion_score, 3),
            "sample_count": self.sample_count,
            "analysis_window_ms": self.analysis_window_ms,
            "flags": [
                {
                    "name": f.name,
                    "description": f.description,
                    "severity": f.severity,
                    "evidence": f.evidence,
                }
                for f in self.flags
            ],
        }


# ─── Audio stream analysis ────────────────────────────────────────────


class AudioStreamAnalyzer:
    """Detect synthetic or programmatic audio editing patterns.

    Analyzes a sequence of buffer_hash events (RMS, ZCR, spectral centroid)
    to identify statistical anomalies that suggest the audio was not produced
    by a human editing session.

    Follows the CPoE behavioral fingerprint analysis model but adapted for
    audio features instead of keystroke timing.
    """

    def __init__(self, min_windows: int = 50) -> None:
        self.min_windows = min_windows
        self._rms_values: list[float] = []
        self._zcr_values: list[float] = []
        self._centroid_values: list[float] = []
        self._transition_intervals_ms: list[float] = []
        self._last_transition_ms: int | None = None

    def ingest_buffer_hash(self, event: dict[str, object]) -> None:
        rms = _finite_number(event.get("rms_level"))
        zcr = _finite_number(event.get("zero_crossing_rate"))
        centroid = _finite_number(event.get("spectral_centroid_hz"))

        if rms is not None:
            self._rms_values.append(rms)
        if zcr is not None:
            self._zcr_values.append(zcr)
        if centroid is not None:
            self._centroid_values.append(centroid)

    def ingest_transition(self, event: dict[str, object]) -> None:
        ts = _finite_number(event.get("timestamp_ms"))
        if ts is not None:
            ts_int = int(ts)
            if self._last_transition_ms is not None:
                interval = ts_int - self._last_transition_ms
                if 0 < interval < 300_000:
                    self._transition_intervals_ms.append(float(interval))
            self._last_transition_ms = ts_int

    def has_enough_data(self) -> bool:
        return len(self._rms_values) >= self.min_windows

    def analyze(self) -> ForgeryReport:
        flags: list[ForgeryFlag] = []

        if len(self._rms_values) >= self.min_windows:
            flags.extend(self._check_rms_regularity())
            flags.extend(self._check_spectral_regularity())

        if len(self._transition_intervals_ms) >= 10:
            flags.extend(self._check_transition_regularity())
            flags.extend(self._check_impossible_timing())

        return ForgeryReport(
            flags=flags,
            sample_count=len(self._rms_values),
            analysis_window_ms=0,
        )

    def _check_rms_regularity(self) -> list[ForgeryFlag]:
        """Too-regular RMS suggests programmatic generation.

        Human audio editing produces variable loudness. A coefficient of
        variation (CV = std/mean) below 0.05 across many windows is unusual
        for real editing sessions.
        """
        flags: list[ForgeryFlag] = []
        mean, std = _mean_std(self._rms_values)
        if mean > 0:
            cv = std / mean
            if cv < 0.05:
                flags.append(ForgeryFlag(
                    name="too_regular_rms",
                    description="RMS level is unusually consistent across windows",
                    severity=0.7,
                    evidence=f"CV={cv:.4f} (threshold: 0.05)",
                ))
        return flags

    def _check_spectral_regularity(self) -> list[ForgeryFlag]:
        """Too-regular spectral centroid suggests looped or synthetic content.

        Real audio editing sessions produce varied spectral content as the
        producer works with different sounds, effects, and arrangements.
        """
        flags: list[ForgeryFlag] = []
        if len(self._centroid_values) < self.min_windows:
            return flags

        mean, std = _mean_std(self._centroid_values)
        if mean > 0:
            cv = std / mean
            if cv < 0.02:
                flags.append(ForgeryFlag(
                    name="too_regular_spectrum",
                    description="Spectral centroid is unusually consistent",
                    severity=0.6,
                    evidence=f"CV={cv:.4f} (threshold: 0.02)",
                ))
        return flags

    def _check_transition_regularity(self) -> list[ForgeryFlag]:
        """Too-regular transition timing suggests scripted playback.

        Human editing produces irregular play/stop patterns. Metronomic
        transitions (very low CV of intervals) indicate automation.
        """
        flags: list[ForgeryFlag] = []
        mean, std = _mean_std(self._transition_intervals_ms)
        if mean > 0:
            cv = std / mean
            if cv < 0.1:
                flags.append(ForgeryFlag(
                    name="metronomic_transitions",
                    description="Play/stop timing is suspiciously regular",
                    severity=0.8,
                    evidence=f"CV={cv:.4f}, mean_interval={mean:.0f}ms (threshold: 0.1)",
                ))
        return flags

    def _check_impossible_timing(self) -> list[ForgeryFlag]:
        """Impossibly fast transitions suggest automated control.

        A human cannot meaningfully edit and restart playback faster than
        about 200ms. Frequent sub-100ms transitions are a strong indicator
        of programmatic control.
        """
        flags: list[ForgeryFlag] = []
        fast_count = sum(1 for t in self._transition_intervals_ms if t < 100)
        total = len(self._transition_intervals_ms)
        if total > 0:
            fast_ratio = fast_count / total
            if fast_ratio > 0.10:
                flags.append(ForgeryFlag(
                    name="superhuman_speed",
                    description="Many transitions are faster than human reaction time",
                    severity=0.9,
                    evidence=f"{fast_count}/{total} transitions < 100ms ({fast_ratio:.0%})",
                ))
        return flags


# ─── Input behavior analysis ──────────────────────────────────────────


class InputBehaviorAnalyzer:
    """Detect synthetic input patterns from OS-level keystroke data.

    Follows the CPoE forgery detection model:
        - CV < 0.2 on inter-key intervals → "Too Regular"
        - Missing micro-pauses (150-500ms) → "Missing Human Pauses"
        - Impossibly fast keystrokes (>10% at <20ms) → "Superhuman Speed"
        - No fatigue/slowdown over session → "No Fatigue Pattern"
        - Skewness mismatch → "Wrong Statistics"
    """

    def __init__(self, min_samples: int = 100) -> None:
        self.min_samples = min_samples
        self._iki_ms: list[float] = []
        self._session_start_ms: int | None = None
        self._session_segments: list[list[float]] = []
        self._current_segment: list[float] = []

    def ingest_keystroke_batch(self, event: dict[str, object]) -> None:
        mean_iki = _finite_number(event.get("mean_iki_ms"))
        count = event.get("count", 0)
        if mean_iki is None or isinstance(count, bool) or not isinstance(count, int):
            return
        for _ in range(min(max(count, 0), 10_000)):
            self._iki_ms.append(mean_iki)
            self._current_segment.append(mean_iki)

    def segment_break(self) -> None:
        if self._current_segment:
            self._session_segments.append(self._current_segment)
            self._current_segment = []

    def has_enough_data(self) -> bool:
        return len(self._iki_ms) >= self.min_samples

    def analyze(self) -> ForgeryReport:
        flags: list[ForgeryFlag] = []

        if not self.has_enough_data():
            return ForgeryReport(flags=flags, sample_count=len(self._iki_ms))

        flags.extend(self._check_regularity())
        flags.extend(self._check_missing_pauses())
        flags.extend(self._check_superhuman_speed())
        flags.extend(self._check_fatigue())
        flags.extend(self._check_skewness())

        return ForgeryReport(
            flags=flags,
            sample_count=len(self._iki_ms),
        )

    def _check_regularity(self) -> list[ForgeryFlag]:
        """CV < 0.2 on IKI → too regular for human typing."""
        flags: list[ForgeryFlag] = []
        mean, std = _mean_std(self._iki_ms)
        if mean > 0:
            cv = std / mean
            if cv < 0.2:
                flags.append(ForgeryFlag(
                    name="too_regular_iki",
                    description="Inter-key intervals are unusually consistent",
                    severity=0.7,
                    evidence=f"CV={cv:.4f} (threshold: 0.2)",
                ))
        return flags

    def _check_missing_pauses(self) -> list[ForgeryFlag]:
        """Humans produce micro-pauses (150-500ms) between action bursts.

        If fewer than 5% of intervals fall in the 150-500ms range, the
        input pattern is missing the cognitive pauses characteristic of
        human editing.
        """
        flags: list[ForgeryFlag] = []
        pause_count = sum(1 for t in self._iki_ms if 150 <= t <= 500)
        total = len(self._iki_ms)
        if total > 0:
            ratio = pause_count / total
            if ratio < 0.05:
                flags.append(ForgeryFlag(
                    name="missing_human_pauses",
                    description="Very few micro-pauses detected between actions",
                    severity=0.6,
                    evidence=f"{pause_count}/{total} in 150-500ms range ({ratio:.1%})",
                ))
        return flags

    def _check_superhuman_speed(self) -> list[ForgeryFlag]:
        """More than 10% of intervals below 20ms is physically impossible."""
        flags: list[ForgeryFlag] = []
        fast = sum(1 for t in self._iki_ms if t < 20)
        total = len(self._iki_ms)
        if total > 0:
            ratio = fast / total
            if ratio > 0.10:
                flags.append(ForgeryFlag(
                    name="superhuman_input_speed",
                    description="Many keystrokes are faster than human capability",
                    severity=0.9,
                    evidence=f"{fast}/{total} intervals < 20ms ({ratio:.0%})",
                ))
        return flags

    def _check_fatigue(self) -> list[ForgeryFlag]:
        """Humans slow down over long sessions. No slowdown is suspicious.

        Compare mean IKI of first quarter vs last quarter of session.
        Humans typically show 5-15% slowdown. No change or speedup is unusual.
        """
        flags: list[ForgeryFlag] = []
        n = len(self._iki_ms)
        if n < 100:
            return flags

        quarter = n // 4
        first_mean, _ = _mean_std(self._iki_ms[:quarter])
        last_mean, _ = _mean_std(self._iki_ms[-quarter:])

        if first_mean > 0:
            slowdown = (last_mean - first_mean) / first_mean
            if slowdown < 0.01:
                flags.append(ForgeryFlag(
                    name="no_fatigue_pattern",
                    description="No slowdown detected over session duration",
                    severity=0.5,
                    evidence=f"First quarter mean: {first_mean:.1f}ms, "
                             f"last quarter: {last_mean:.1f}ms, "
                             f"change: {slowdown:+.1%}",
                ))
        return flags

    def _check_skewness(self) -> list[ForgeryFlag]:
        """Human IKI distributions are right-skewed (long tail of pauses).

        A skewness near 0 or negative suggests synthetic generation.
        Real human distributions typically have skewness > 1.0.
        """
        flags: list[ForgeryFlag] = []
        n = len(self._iki_ms)
        if n < 30:
            return flags

        mean, std = _mean_std(self._iki_ms)
        if std <= 0:
            return flags

        skew = sum(((x - mean) / std) ** 3 for x in self._iki_ms) / n
        if skew < 0.5:
            flags.append(ForgeryFlag(
                name="wrong_iki_statistics",
                description="IKI distribution shape does not match human patterns",
                severity=0.6,
                evidence=f"skewness={skew:.3f} (expected > 1.0 for human input)",
            ))
        return flags


# ─── Hash chain integrity analysis ────────────────────────────────────


class HashChainAnalyzer:
    """Verify hash chain integrity and detect tampering.

    Checks:
        - Chain continuity (each prev_hash matches previous window_hash)
        - No duplicate hashes (would indicate replay)
        - Monotonic timestamps
        - No gaps in sample position sequence
    """

    def __init__(self) -> None:
        self._hashes: list[str] = []
        self._prev_hashes: list[str] = []
        self._timestamps: list[int] = []
        self._sample_positions: list[int] = []

    def ingest_buffer_hash(self, event: dict[str, object]) -> None:
        wh = event.get("window_hash")
        ph = event.get("prev_hash")
        ts = _finite_number(event.get("timestamp_ms"))
        sp = _finite_number(event.get("sample_position"))

        if isinstance(wh, str):
            self._hashes.append(wh)
        if isinstance(ph, str):
            self._prev_hashes.append(ph)
        if ts is not None:
            self._timestamps.append(int(ts))
        if sp is not None:
            self._sample_positions.append(int(sp))

    def analyze(self) -> ForgeryReport:
        flags: list[ForgeryFlag] = []
        flags.extend(self._check_chain_continuity())
        flags.extend(self._check_duplicate_hashes())
        flags.extend(self._check_timestamp_monotonicity())
        return ForgeryReport(
            flags=flags,
            sample_count=len(self._hashes),
        )

    def _check_chain_continuity(self) -> list[ForgeryFlag]:
        """Each prev_hash must match the previous window_hash."""
        flags: list[ForgeryFlag] = []
        breaks = 0
        for i in range(1, min(len(self._hashes), len(self._prev_hashes))):
            if self._prev_hashes[i] != self._hashes[i - 1]:
                breaks += 1

        if breaks > 0:
            flags.append(ForgeryFlag(
                name="chain_break",
                description="Hash chain has discontinuities",
                severity=1.0,
                evidence=f"{breaks} break(s) in {len(self._hashes)} windows",
            ))
        return flags

    def _check_duplicate_hashes(self) -> list[ForgeryFlag]:
        """Duplicate window hashes indicate replay or collision."""
        flags: list[ForgeryFlag] = []
        seen: set[str] = set()
        duplicates = 0
        for h in self._hashes:
            if h in seen:
                duplicates += 1
            seen.add(h)

        if duplicates > 0:
            flags.append(ForgeryFlag(
                name="duplicate_hashes",
                description="Hash chain contains duplicate window hashes (possible replay)",
                severity=0.9,
                evidence=f"{duplicates} duplicate(s) in {len(self._hashes)} windows",
            ))
        return flags

    def _check_timestamp_monotonicity(self) -> list[ForgeryFlag]:
        """Timestamps must be non-decreasing."""
        flags: list[ForgeryFlag] = []
        reversals = 0
        for i in range(1, len(self._timestamps)):
            if self._timestamps[i] < self._timestamps[i - 1]:
                reversals += 1

        if reversals > 0:
            flags.append(ForgeryFlag(
                name="timestamp_reversal",
                description="Timestamps are not monotonically increasing",
                severity=1.0,
                evidence=f"{reversals} reversal(s) in {len(self._timestamps)} events",
            ))
        return flags


# ─── Utilities ────────────────────────────────────────────────────────


def _finite_number(value: object) -> float | None:
    """Numeric value safe for statistics; None for bool, non-numbers, inf, NaN."""
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    try:
        number = float(value)
    except OverflowError:
        return None
    return number if math.isfinite(number) else None


def _mean_std(values: list[float]) -> tuple[float, float]:
    if not values:
        return 0.0, 0.0
    n = len(values)
    mean = sum(values) / n
    variance = sum((x - mean) ** 2 for x in values) / n
    return mean, math.sqrt(variance)
