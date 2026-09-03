"""Kill criterion K0's verdict logic, on constructed measurements.

Separated from the DSP: whether a swept-sine deconvolution recovers RT60 and DRR is tested in
test_sweep_math.py against a designed reference. What is tested here is the decision K0 makes on
whatever those numbers are, because that decision stops or continues the programme.
"""

from __future__ import annotations

import numpy as np

from capture_rig.config import Room
from capture_rig.rooms import RoomMeasurement, evaluate_k0
from capture_rig.sweep import ExtractedResponse

ROOMS = (
    Room(id="treated_small", description="", role="treated", rt60_target_seconds=0.25, spl_calibration=None),
    Room(id="office", description="", role="office", rt60_target_seconds=0.45, spl_calibration=None),
    Room(id="live_hard", description="", role="live", rt60_target_seconds=0.80, spl_calibration=None),
)


def _measurement(room_id: str, rt60: float | None, drr: float | None, distance: float = 1.0,
                 trustworthy: bool = True) -> RoomMeasurement:
    response = ExtractedResponse(
        ir=np.zeros(8), sample_rate=48000, direct_index=0, bulk_delay_seconds=0.0,
        linear_peak_abs=1.0, linear_energy=1.0, noise_rms=1e-6,
    )
    return RoomMeasurement(
        task_id=f"rir_sweep/{room_id}/cell/sweep/rep00",
        room_id=room_id,
        speaker_id="monitor",
        microphone_id="usb",
        distance_m=distance,
        response=response,
        rt60={"seconds": rt60, "trustworthy": trustworthy, "convention": "schroeder_t30"},
        octave_rt60={},
        rt60_mid_seconds=rt60,
        drr=drr,
    )


def _grid(rt60s: dict[str, float], drrs: dict[str, float], **kwargs) -> list[RoomMeasurement]:
    return [_measurement(room, rt60s[room], drrs[room], **kwargs) for room in rt60s]


def test_k0_passes_when_the_treated_room_is_inside_its_envelope():
    verdict = evaluate_k0(
        _grid({"treated_small": 0.25, "office": 0.45, "live_hard": 0.80},
              {"treated_small": 9.0, "office": 4.0, "live_hard": -1.0}),
        ROOMS,
    )
    assert verdict["verdict"] == "pass"
    assert verdict["reasons"] == []
    assert verdict["rooms"]["treated_small"]["median_rt60_seconds"] == 0.25
    assert verdict["untrustworthy_measurements"] == 0


def test_k0_kills_when_the_treated_room_exceeds_zero_point_six_seconds():
    verdict = evaluate_k0(
        _grid({"treated_small": 0.72, "office": 0.45, "live_hard": 0.80},
              {"treated_small": 9.0, "office": 4.0, "live_hard": -1.0}),
        ROOMS,
    )
    assert verdict["verdict"] == "kill"
    assert any("above the 0.6 s limit" in reason for reason in verdict["reasons"])


def test_k0_kills_only_when_every_room_is_below_zero_db_drr():
    below_everywhere = evaluate_k0(
        _grid({"treated_small": 0.25, "office": 0.45, "live_hard": 0.80},
              {"treated_small": -2.0, "office": -4.0, "live_hard": -9.0}),
        ROOMS,
    )
    assert below_everywhere["verdict"] == "kill"
    assert any("below 0.0 dB in all" in reason for reason in below_everywhere["reasons"])

    one_room_positive = evaluate_k0(
        _grid({"treated_small": 0.25, "office": 0.45, "live_hard": 0.80},
              {"treated_small": 1.0, "office": -4.0, "live_hard": -9.0}),
        ROOMS,
    )
    assert one_room_positive["verdict"] == "pass"


def test_k0_is_unevaluated_without_a_treated_room_measured_at_one_metre():
    verdict = evaluate_k0(
        _grid({"office": 0.45, "live_hard": 0.80}, {"office": 4.0, "live_hard": -1.0}),
        ROOMS,
    )
    assert verdict["verdict"] == "unevaluated"
    assert not verdict["fires"]
    assert any("role 'treated'" in reason for reason in verdict["reasons"])


def test_k0_ignores_measurements_taken_at_other_distances():
    verdict = evaluate_k0(
        _grid({"treated_small": 0.9, "office": 0.9, "live_hard": 0.9},
              {"treated_small": -9.0, "office": -9.0, "live_hard": -9.0}, distance=3.0),
        ROOMS,
    )
    assert verdict["verdict"] == "unevaluated"
    assert verdict["rooms"] == {}


def test_k0_reports_untrustworthy_measurements_so_the_verdict_carries_its_caveat():
    verdict = evaluate_k0(
        _grid({"treated_small": 0.25, "office": 0.45, "live_hard": 0.80},
              {"treated_small": 9.0, "office": 4.0, "live_hard": -1.0}, trustworthy=False),
        ROOMS,
    )
    assert verdict["untrustworthy_measurements"] == 3
    assert "provisional" in verdict["measurement_quality_note"]
