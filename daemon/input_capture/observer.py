from __future__ import annotations

import argparse
import logging
from pathlib import Path

log = logging.getLogger(__name__)


class InputObserver:
    """Captures OS-level keyboard and mouse events while the DAW is focused.

    On macOS this will use CGEventTap (Quartz Event Services) to passively
    observe input events systemwide, filtering to the target DAW window.

    Platform requirements:
        macOS  - Accessibility permission (kAXTrustedCheckOptionPrompt)
        Linux  - evdev access or X11 event capture (future)
        Windows - SetWindowsHookEx WH_KEYBOARD_LL / WH_MOUSE_LL (future)
    """

    def __init__(
        self,
        target_bundle_id: str = "com.ableton.live",
        evidence_path: Path = Path("evidence/input_events.jsonl"),
        zone_grid: tuple[int, int] = (8, 6),
    ) -> None:
        self.target_bundle_id = target_bundle_id
        self.evidence_path = evidence_path.expanduser()
        self.zone_grid = zone_grid
        self._running = False

    def check_accessibility_permission(self) -> bool:
        """Return True if the process has Accessibility permission on macOS.

        Stub: actual implementation will call AXIsProcessTrustedWithOptions
        via pyobjc or ctypes.
        """
        raise NotImplementedError("CGEventTap integration not yet implemented")

    def start(self) -> None:
        """Register the event tap and begin capturing.

        Stub: actual implementation will:
        1. Check accessibility permission (degrade gracefully if denied).
        2. Create a CGEventTap for kCGEventKeyDown, kCGEventKeyUp,
           kCGEventLeftMouseDown, kCGEventLeftMouseUp, kCGEventLeftMouseDragged,
           kCGEventScrollWheel.
        3. Filter events to those targeting the DAW window (by bundle ID).
        4. Extract structured data (keycode, modifiers, zone-bucketed coords).
        5. Feed events to the shortcut classifier.
        6. Write evidence JSONL.
        """
        raise NotImplementedError("CGEventTap integration not yet implemented")

    def stop(self) -> None:
        """Remove the event tap and stop capturing."""
        self._running = False


class ShortcutClassifier:
    """Maps (modifier_flags, keycode) tuples to probable DAW edit operations.

    The mapping is DAW-specific. This scaffold provides the Ableton Live
    mapping; additional DAWs can be added as subclasses or config files.
    """

    ABLETON_SHORTCUTS: dict[tuple[int, int], tuple[str, float]] = {
        # (modifier_mask, keycode): (operation, base_confidence)
        # Modifier masks: cmd=0x100000, shift=0x20000, opt=0x80000
        # Keycodes: c=0x08, v=0x09, x=0x07, z=0x06, d=0x02, e=0x0E, j=0x26
        (0x100000, 0x08): ("copy", 0.85),
        (0x100000, 0x09): ("paste", 0.85),
        (0x100000, 0x07): ("cut", 0.85),
        (0x100000, 0x06): ("undo", 0.90),
        (0x120000, 0x06): ("redo", 0.90),        # cmd+shift+z
        (0x100000, 0x02): ("duplicate", 0.85),
        (0x100000, 0x0E): ("split_clip", 0.85),
        (0x100000, 0x26): ("consolidate", 0.80),
        (0x100000, 0x25): ("toggle_loop", 0.80),  # cmd+l
        (0, 0x33): ("delete", 0.80),               # backspace
        (0, 0x75): ("delete", 0.80),               # forward delete
        (0, 0x30): ("toggle_view", 0.75),           # tab
    }

    def __init__(self, daw: str = "ableton") -> None:
        self.daw = daw
        if daw == "ableton":
            self._map = self.ABLETON_SHORTCUTS
        else:
            self._map = {}

    def classify(self, modifier_flags: int, keycode: int) -> tuple[str, float] | None:
        """Return (operation, confidence) or None if the shortcut is unrecognized."""
        return self._map.get((modifier_flags, keycode))


class BehavioralAccumulator:
    """Accumulates keystroke timing data for behavioral fingerprinting.

    Follows the CPoE pattern: inter-key interval (IKI) distribution, dwell
    time, flight time, and zone transition profile.

    Stub: actual implementation will compute running statistics and emit
    periodic fingerprint events.
    """

    def __init__(self, min_samples: int = 100) -> None:
        self.min_samples = min_samples
        self._iki_samples: list[float] = []
        self._dwell_samples: list[float] = []
        self._last_key_down_ns: int | None = None
        self._last_key_up_ns: int | None = None

    def record_key_down(self, timestamp_ns: int, keycode: int) -> None:
        if self._last_key_up_ns is not None:
            flight_ms = (timestamp_ns - self._last_key_up_ns) / 1_000_000
            if 0 < flight_ms < 5000:
                self._iki_samples.append(flight_ms)
        self._last_key_down_ns = timestamp_ns

    def record_key_up(self, timestamp_ns: int, keycode: int) -> None:
        if self._last_key_down_ns is not None:
            dwell_ms = (timestamp_ns - self._last_key_down_ns) / 1_000_000
            if 0 < dwell_ms < 2000:
                self._dwell_samples.append(dwell_ms)
        self._last_key_up_ns = timestamp_ns

    def has_enough_samples(self) -> bool:
        return len(self._iki_samples) >= self.min_samples

    def compute_fingerprint(self) -> dict[str, object] | None:
        """Return a behavioral fingerprint dict, or None if insufficient data.

        Stub: full implementation will compute mean, std, skewness, kurtosis
        of IKI distribution, plus pause signature and zone profile.
        """
        if not self.has_enough_samples():
            return None
        raise NotImplementedError("Behavioral fingerprint computation not yet implemented")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Capture OS-level input events while the DAW is focused.",
    )
    parser.add_argument("--bundle-id", default="com.ableton.live", help="Target DAW bundle ID.")
    parser.add_argument(
        "--evidence-file",
        type=Path,
        default=Path("evidence/input_events.jsonl"),
        help="JSONL output path.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    logging.basicConfig(level=logging.INFO, format="%(levelname)s: %(message)s")
    args = parse_args(argv)
    observer = InputObserver(
        target_bundle_id=args.bundle_id,
        evidence_path=args.evidence_file,
    )
    log.info(
        "Input capture scaffold loaded. CGEventTap integration pending. "
        "Target: %s",
        observer.target_bundle_id,
    )
    return 0
