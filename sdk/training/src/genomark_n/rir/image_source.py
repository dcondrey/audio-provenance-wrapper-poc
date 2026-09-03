"""Image-source room impulse responses, written here so RT60 and geometry are training parameters
rather than a data dependency.

Three things this implementation does that a naive image-source model gets wrong, each of which
would leak a cue a learned encoder can key on and a real room does not provide:

  1. EYRING, NOT SABINE. Sabine's alpha = 0.161 V / (S RT60) exceeds 1 for a small room at
     RT60 ~ 0.25 s, which is not a physical absorption coefficient. Eyring inverts to
     alpha = 1 - exp(-0.161 V / (S RT60)) and stays in (0, 1). The clamp is asserted, not silent:
     a bound that binds means the synthesized room is not the room the config asked for.
  2. FRACTIONAL DELAY. Image sources land on non-integer sample delays. Rounding to the nearest
     sample produces comb artifacts that vanish in a real room, so taps are placed with a
     windowed-sinc fractional delay.
  3. DIRECT AND LATE RETURNED SEPARATELY. Spec 6 D6 scales the direct tap and the tail
     independently to hit a sampled direct-to-reverberant ratio. DRR is the distance proxy and the
     axis the operating envelope is reported against; one flat IR has thrown that control away.
"""

import math
from dataclasses import dataclass

import numpy as np
from scipy.signal import butter, sosfilt

SPEED_OF_SOUND = 343.0
LN_10_POW_6 = 6.907755278982137


class RoomGeometryError(ValueError):
    """The requested room and RT60 are not jointly realisable."""


@dataclass(frozen=True)
class RirPair:
    """A room response split at the boundary spec 6 D6 needs to control."""

    direct: np.ndarray
    late: np.ndarray
    sample_rate: int
    rt60_seconds: float
    measured_rt60_seconds: float | None
    absorption: float
    source_distance_m: float
    room_dims_m: tuple[float, float, float]
    images_used: int

    def at_drr(self, drr_db: float) -> np.ndarray:
        """Combine into one IR whose direct-to-reverberant ratio is `drr_db`, unit total energy."""
        direct_energy = float((self.direct**2).sum())
        late_energy = float((self.late**2).sum())
        if direct_energy <= 0.0:
            return self.late / max(math.sqrt(late_energy), 1e-12)
        if late_energy <= 0.0:
            return self.direct / max(math.sqrt(direct_energy), 1e-12)
        target = 10.0 ** (drr_db / 10.0)
        late_scale = math.sqrt(direct_energy / (target * late_energy))
        combined = self.direct + late_scale * self.late
        energy = float((combined**2).sum())
        return combined / max(math.sqrt(energy), 1e-12)

    def measured_drr_db(self) -> float:
        direct_energy = float((self.direct**2).sum())
        late_energy = float((self.late**2).sum())
        return 10.0 * math.log10(max(direct_energy, 1e-30) / max(late_energy, 1e-30))


def eyring_absorption(volume: float, surface: float, rt60: float) -> float:
    """alpha = 1 - exp(-0.161 V / (S RT60)). Always in (0, 1) for positive inputs."""
    if volume <= 0.0 or surface <= 0.0 or rt60 <= 0.0:
        raise RoomGeometryError("volume, surface and RT60 must all be positive")
    return 1.0 - math.exp(-0.161 * volume / (surface * rt60))


def _fractional_delay_taps(delay: float, taps: int) -> tuple[int, np.ndarray]:
    """Windowed-sinc fractional delay. Returns the first sample index and the tap values."""
    half = taps // 2
    base = int(math.floor(delay))
    frac = delay - base
    n = np.arange(-half, taps - half, dtype=np.float64)
    sinc = np.sinc(n - frac)
    window = 0.5 - 0.5 * np.cos(2.0 * math.pi * (np.arange(taps) + 0.5) / taps)
    return base - half, sinc * window


@dataclass(frozen=True)
class ImageSourceRoom:
    room_dims_m: tuple[float, float, float]
    source_m: tuple[float, float, float]
    receiver_m: tuple[float, float, float]
    rt60_seconds: float
    max_order: int = 12
    hf_rt60_ratio: float = 0.55
    crossover_hz: float = 1500.0
    fractional_delay_taps: int = 33

    def validate(self) -> None:
        for name, point in (("source", self.source_m), ("receiver", self.receiver_m)):
            for axis, (value, limit) in enumerate(zip(point, self.room_dims_m, strict=True)):
                if not 0.05 <= value <= limit - 0.05:
                    raise RoomGeometryError(
                        f"{name} axis {axis} at {value:.3f} m is outside the room (0.05..{limit - 0.05:.3f})"
                    )
        if self.rt60_seconds <= 0.0:
            raise RoomGeometryError("rt60 must be positive")

    @property
    def source_distance_m(self) -> float:
        return float(np.linalg.norm(np.array(self.source_m) - np.array(self.receiver_m)))


def synthesise_rir(
    room: ImageSourceRoom,
    sample_rate: int,
    max_seconds: float = 1.5,
    rng: np.random.Generator | None = None,
) -> RirPair:
    room.validate()
    rng = rng or np.random.default_rng(0)
    dims = np.array(room.room_dims_m, dtype=np.float64)
    volume = float(dims.prod())
    surface = float(2.0 * (dims[0] * dims[1] + dims[1] * dims[2] + dims[0] * dims[2]))
    absorption = eyring_absorption(volume, surface, room.rt60_seconds)
    if not 1e-4 < absorption < 1.0 - 1e-6:
        raise RoomGeometryError(
            f"Eyring absorption {absorption:.6f} for RT60 {room.rt60_seconds:.3f} s in a "
            f"{dims[0]:.2f}x{dims[1]:.2f}x{dims[2]:.2f} m room is not physical; the synthesized "
            "room would not be the room the config asked for"
        )
    reflection = math.sqrt(1.0 - absorption)

    wanted_seconds = room.rt60_seconds * 1.3
    truncated = wanted_seconds > max_seconds
    length = int(round(min(wanted_seconds, max_seconds) * sample_rate))
    length = max(length, 64)
    taps = room.fractional_delay_taps
    padded = length + taps
    direct = np.zeros(padded, dtype=np.float64)
    late = np.zeros(padded, dtype=np.float64)

    source = np.array(room.source_m, dtype=np.float64)
    receiver = np.array(room.receiver_m, dtype=np.float64)
    order = room.max_order
    grid = np.arange(-order, order + 1, dtype=np.float64)
    mx, my, mz = np.meshgrid(grid, grid, grid, indexing="ij")
    m_stack = np.stack([mx.ravel(), my.ravel(), mz.ravel()], axis=1)

    images_used = 0
    max_delay = length - 1
    for px in (0, 1):
        for py in (0, 1):
            for pz in (0, 1):
                parity = np.array([px, py, pz], dtype=np.float64)
                image = (1.0 - 2.0 * parity) * source + 2.0 * m_stack * dims
                offsets = image - receiver
                distance = np.linalg.norm(offsets, axis=1)
                delay = distance / SPEED_OF_SOUND * sample_rate
                reflections = (
                    np.abs(m_stack - parity).sum(axis=1) + np.abs(m_stack).sum(axis=1)
                )
                amplitude = reflection**reflections / np.maximum(4.0 * math.pi * distance, 1e-6)
                keep = (delay <= max_delay) & (amplitude > 1e-9)
                if not keep.any():
                    continue
                is_direct = (reflections == 0) & (px == 0) & (py == 0) & (pz == 0)
                for index in np.nonzero(keep)[0]:
                    start, values = _fractional_delay_taps(float(delay[index]), taps)
                    if start < 0:
                        values = values[-start:]
                        start = 0
                    stop = min(start + values.size, padded)
                    if stop <= start:
                        continue
                    target = direct if is_direct[index] else late
                    target[start:stop] += amplitude[index] * values[: stop - start]
                    images_used += 1

    direct = direct[:length]
    late = late[:length]
    time = np.arange(length, dtype=np.float64) / sample_rate

    # IMPORTANT: a rectangular image-source field does NOT decay at the Eyring rate. Reflection
    # count per metre varies with direction (axial paths hit far fewer walls than oblique ones), so
    # the direction-averaged energy decay is slower than beta^(2<N>) by Jensen's inequality. Left
    # uncorrected this room measures ~0.55 s of T30 when the config asked for 0.40 s, and every
    # RT60 the model card reports would be a number nobody measured. The geometry sets the
    # reflection PATTERN; this envelope sets the decay RATE to the one that was asked for, and the
    # achieved T30 is measured back and carried on the result.
    natural = measure_rt60(late, sample_rate)
    if natural is not None and natural > 1e-3:
        late = late * np.exp(-LN_10_POW_6 * time * (1.0 / room.rt60_seconds - 1.0 / natural))

    # Frequency-dependent decay, mirroring audio-provenance-bench's two-band synthesizer: the high band
    # decays at hf_rt60_ratio of the broadband RT60, as a real room's air and surface absorption make it.
    hf_rt60 = max(room.rt60_seconds * room.hf_rt60_ratio, 1e-3)
    extra = np.exp(-LN_10_POW_6 * time * (1.0 / hf_rt60 - 1.0 / room.rt60_seconds))
    low_sos = butter(2, room.crossover_hz, btype="low", fs=sample_rate, output="sos")
    high_sos = butter(2, room.crossover_hz, btype="high", fs=sample_rate, output="sos")
    late = np.asarray(sosfilt(low_sos, late)) + np.asarray(sosfilt(high_sos, late)) * extra

    measured = measure_rt60(late, sample_rate)
    for _ in range(0 if truncated else 3):
        if measured is None or abs(measured - room.rt60_seconds) / room.rt60_seconds < 0.02:
            break
        late = late * np.exp(-LN_10_POW_6 * time * (1.0 / room.rt60_seconds - 1.0 / measured))
        measured = measure_rt60(late, sample_rate)

    if truncated:
        # A Schroeder curve computed on a response cut off before the decay finishes is not a
        # measurement of anything, so it is not reported as one and the correction loop that would
        # fight it does not run.
        measured = None
    return RirPair(
        direct=direct.astype(np.float32),
        late=late.astype(np.float32),
        sample_rate=sample_rate,
        rt60_seconds=room.rt60_seconds,
        measured_rt60_seconds=measured,
        absorption=absorption,
        source_distance_m=room.source_distance_m,
        room_dims_m=room.room_dims_m,
        images_used=images_used,
    )


def measure_rt60(ir: np.ndarray, sample_rate: int) -> float | None:
    """Schroeder backward integration, T30 extrapolated to 60 dB. Returns None when the decay
    never falls 35 dB, which is the honest answer for a truncated response."""
    energy = np.asarray(ir, dtype=np.float64) ** 2
    total = energy[::-1].cumsum()[::-1]
    if total[0] <= 0.0:
        return None
    curve = 10.0 * np.log10(np.maximum(total / total[0], 1e-30))
    try:
        start = int(np.nonzero(curve <= -5.0)[0][0])
        stop = int(np.nonzero(curve <= -35.0)[0][0])
    except IndexError:
        return None
    if stop <= start:
        return None
    times = np.arange(start, stop, dtype=np.float64) / sample_rate
    slope, _ = np.polyfit(times, curve[start:stop], 1)
    if slope >= 0.0:
        return None
    return float(-60.0 / slope)


def sample_room(
    rng: np.random.Generator,
    dims_min: tuple[float, float, float],
    dims_max: tuple[float, float, float],
    rt60_range: tuple[float, float],
    distance_range: tuple[float, float],
    max_order: int,
    fractional_delay_taps: int,
    hf_rt60_ratio: float,
) -> ImageSourceRoom:
    """Draw a realisable room. Retries only the source/receiver placement, never the RT60, so the
    sampled RT60 distribution is the one the config states."""
    dim_x, dim_y, dim_z = (float(rng.uniform(lo, hi))
                          for lo, hi in zip(dims_min, dims_max, strict=True))
    dims = (dim_x, dim_y, dim_z)
    rt60 = float(rng.uniform(*rt60_range))
    distance = float(rng.uniform(*distance_range))
    max_distance = math.sqrt(sum((d - 0.2) ** 2 for d in dims))
    distance = min(distance, max_distance * 0.8)
    for _ in range(64):
        rec_x, rec_y, rec_z = (float(rng.uniform(0.4, d - 0.4)) for d in dims)
        receiver = (rec_x, rec_y, rec_z)
        direction = rng.normal(size=3)
        direction /= max(float(np.linalg.norm(direction)), 1e-9)
        src_x, src_y, src_z = (float(r + distance * u)
                               for r, u in zip(receiver, direction, strict=True))
        source = (src_x, src_y, src_z)
        if all(0.15 <= s <= d - 0.15 for s, d in zip(source, dims, strict=True)):
            return ImageSourceRoom(
                room_dims_m=dims,
                source_m=source,
                receiver_m=receiver,
                rt60_seconds=rt60,
                max_order=max_order,
                fractional_delay_taps=fractional_delay_taps,
                hf_rt60_ratio=hf_rt60_ratio,
            )
    raise RoomGeometryError(
        f"could not place a source {distance:.2f} m from a receiver in a "
        f"{dims[0]:.2f}x{dims[1]:.2f}x{dims[2]:.2f} m room"
    )
