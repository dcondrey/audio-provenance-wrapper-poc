from __future__ import annotations

import argparse
import json
import logging
import time
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class RegionBounds:
    x: int
    y: int
    width: int
    height: int


@dataclass(frozen=True)
class RegionFeatures:
    phash: str
    edge_density: float
    horizontal_structure: bytes
    vertical_structure: bytes

    def distance(self, other: RegionFeatures) -> int:
        """Hamming distance between perceptual hashes (0-64 bits)."""
        if len(self.phash) != len(other.phash):
            return 64
        xor_val = int(self.phash, 16) ^ int(other.phash, 16)
        return bin(xor_val).count("1")


@dataclass
class FrameObservation:
    timestamp_ms: int
    arrangement: RegionFeatures | None = None
    mixer: RegionFeatures | None = None
    transport: RegionFeatures | None = None
    classification: str = "unknown"


@dataclass
class CaptureRateController:
    """Adjusts capture frequency based on activity from other layers.

    Rates (captures per second):
        active editing  - 2.0   (every 500ms)
        normal          - 0.5   (every 2000ms)
        idle            - 0.2   (every 5000ms)
        unfocused       - 0     (no capture)
    """

    base_interval_ms: int = 2000
    active_interval_ms: int = 500
    idle_interval_ms: int = 5000
    idle_timeout_ms: int = 10000

    _last_input_event_ms: int = 0
    _last_audio_event_ms: int = 0
    _daw_focused: bool = False

    def notify_input_event(self, timestamp_ms: int) -> None:
        self._last_input_event_ms = timestamp_ms

    def notify_audio_event(self, timestamp_ms: int) -> None:
        self._last_audio_event_ms = timestamp_ms

    def notify_focus(self, focused: bool) -> None:
        self._daw_focused = focused

    def current_interval_ms(self) -> int:
        if not self._daw_focused:
            return 0

        now_ms = int(time.time() * 1000)
        last_activity = max(self._last_input_event_ms, self._last_audio_event_ms)
        elapsed = now_ms - last_activity

        if elapsed < 1000:
            return self.active_interval_ms
        if elapsed > self.idle_timeout_ms:
            return self.idle_interval_ms
        return self.base_interval_ms


def capture_daw_window(bundle_id: str) -> bytes | None:
    """Capture the DAW window as raw RGBA pixel data at reduced resolution.

    Stub: actual implementation will use CGWindowListCreateImage (macOS) to
    capture only the target window at 50% resolution. Returns raw RGBA bytes
    or None if the window is not visible.

    The pixel buffer is transient and must NOT be written to disk.
    """
    raise NotImplementedError("Screen capture integration not yet implemented")


def downscale(rgba_data: bytes, src_width: int, src_height: int, factor: int = 2) -> tuple[bytes, int, int]:
    """Downscale RGBA pixel data by averaging factor x factor blocks.

    Stub: actual implementation operates on raw bytes without external deps.
    Returns (downscaled_rgba, new_width, new_height).
    """
    raise NotImplementedError("Downscale not yet implemented")


def extract_region(rgba_data: bytes, img_width: int, bounds: RegionBounds) -> bytes:
    """Extract a rectangular region from RGBA pixel data.

    Stub: copies rows from bounds.y to bounds.y+bounds.height,
    columns from bounds.x to bounds.x+bounds.width.
    """
    raise NotImplementedError("Region extraction not yet implemented")


def compute_phash(rgba_data: bytes, width: int, height: int) -> str:
    """Compute a 64-bit perceptual hash of an image region.

    Algorithm (DCT-based pHash):
    1. Convert to grayscale (single channel).
    2. Resize to 32x32 using area averaging.
    3. Compute 2D DCT.
    4. Keep top-left 8x8 coefficients (low frequencies).
    5. Compute median of 64 coefficients.
    6. Threshold: bit=1 if coeff > median, else 0.
    7. Return as 16-char hex string (64 bits).

    Stub: actual implementation uses pure Python DCT (no external deps).
    For the POC, a simpler average-hash is acceptable as a starting point.
    """
    raise NotImplementedError("Perceptual hash not yet implemented")


def compute_edge_density(rgba_data: bytes, width: int, height: int) -> float:
    """Compute edge density as a single scalar [0.0, 1.0].

    Algorithm:
    1. Convert to grayscale.
    2. Apply 3x3 Sobel operators (horizontal and vertical).
    3. Compute gradient magnitude per pixel.
    4. Return mean gradient magnitude normalized to [0, 1].

    Stub: returns 0.0.
    """
    raise NotImplementedError("Edge density not yet implemented")


def compute_structure_hashes(rgba_data: bytes, width: int, height: int) -> tuple[bytes, bytes]:
    """Compute row-averaged and column-averaged luminance vectors.

    Horizontal: one luminance value per row (height bytes).
    Vertical: one luminance value per column (width bytes).

    These vectors capture the coarse spatial layout without storing pixels.

    Stub: returns empty bytes.
    """
    raise NotImplementedError("Structure hashes not yet implemented")


def extract_features(rgba_data: bytes, width: int, height: int) -> RegionFeatures:
    """Full feature extraction pipeline for a single region.

    Stub: calls compute_phash, compute_edge_density, compute_structure_hashes.
    """
    return RegionFeatures(
        phash=compute_phash(rgba_data, width, height),
        edge_density=compute_edge_density(rgba_data, width, height),
        horizontal_structure=compute_structure_hashes(rgba_data, width, height)[0],
        vertical_structure=compute_structure_hashes(rgba_data, width, height)[1],
    )


def classify_changes(
    prev: FrameObservation | None,
    current: FrameObservation,
    arrangement_threshold: int = 8,
    mixer_threshold: int = 6,
) -> str:
    """Classify the type of visual change between two frames.

    Returns one of: "no_change", "arrangement_changed", "mixer_changed",
    "view_switched", "multiple_changed".
    """
    if prev is None:
        return "initial_capture"

    arr_changed = False
    mix_changed = False

    if prev.arrangement is not None and current.arrangement is not None:
        arr_changed = current.arrangement.distance(prev.arrangement) > arrangement_threshold

    if prev.mixer is not None and current.mixer is not None:
        mix_changed = current.mixer.distance(prev.mixer) > mixer_threshold

    if arr_changed and mix_changed:
        return "view_switched"
    if arr_changed:
        return "arrangement_changed"
    if mix_changed:
        return "mixer_changed"
    return "no_change"


class ScreenObserver:
    """Periodically captures the DAW window and extracts feature vectors.

    No pixel data is stored. The capture buffer is transient and discarded
    after feature extraction (< 50ms lifetime per frame).
    """

    def __init__(
        self,
        target_bundle_id: str = "com.ableton.live",
        evidence_path: Path = Path("evidence/screen_events.jsonl"),
        arrangement_bounds: RegionBounds | None = None,
        mixer_bounds: RegionBounds | None = None,
        transport_bounds: RegionBounds | None = None,
    ) -> None:
        self.target_bundle_id = target_bundle_id
        self.evidence_path = evidence_path.expanduser()
        self.arrangement_bounds = arrangement_bounds
        self.mixer_bounds = mixer_bounds
        self.transport_bounds = transport_bounds
        self.rate_controller = CaptureRateController()
        self._previous_frame: FrameObservation | None = None

    def capture_and_analyze(self) -> FrameObservation | None:
        """Capture one frame, extract features, classify, discard pixels.

        Stub: actual implementation will:
        1. capture_daw_window() -> raw RGBA
        2. downscale() -> reduced RGBA
        3. For each configured region: extract_region() -> region RGBA
        4. For each region: extract_features() -> RegionFeatures
        5. classify_changes() -> classification string
        6. Discard all pixel buffers (they go out of scope)
        7. Return FrameObservation (feature vectors only, ~500 bytes)
        """
        raise NotImplementedError("Screen capture integration not yet implemented")

    def run_forever(self) -> None:
        """Capture loop with adaptive rate control.

        Stub: the control flow is complete; capture_and_analyze raises
        NotImplementedError until platform capture is integrated.
        """
        log.info(
            "Screen observer targeting %s; writing %s",
            self.target_bundle_id,
            self.evidence_path,
        )

        while True:
            interval = self.rate_controller.current_interval_ms()
            if interval <= 0:
                time.sleep(1.0)
                continue

            observation = self.capture_and_analyze()
            if observation is not None and observation.classification != "no_change":
                log.info("Screen: %s", observation.classification)
                self._previous_frame = observation

            time.sleep(interval / 1000.0)


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Observe the DAW window via periodic screen capture and feature extraction.",
    )
    parser.add_argument("--bundle-id", default="com.ableton.live", help="Target DAW bundle ID.")
    parser.add_argument(
        "--evidence-file",
        type=Path,
        default=Path("evidence/screen_events.jsonl"),
        help="JSONL output path.",
    )
    return parser.parse_args(argv)


def platform_support(system: str | None = None) -> dict[str, object]:
    """Report whether DAW window capture works on this OS.

    No OS has a capture backend integrated (CGWindowListCreateImage / X11 / Windows.Graphics.Capture are
    not wired), so every platform reports unsupported and nothing is captured.
    """
    import platform

    system = system or platform.system() or "unknown"
    return {
        "layer": "screen_observer",
        "platform": system,
        "supported": False,
        "status": "unsupported_platform",
        "proof_level": "unknown_unobserved",
        "reason": f"no DAW window capture backend is implemented for {system}",
    }


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    args = parse_args(argv)
    status = platform_support()
    log.warning("%s (target %s); nothing will be captured", status["reason"], args.bundle_id)
    print(json.dumps(status))
    return 2
