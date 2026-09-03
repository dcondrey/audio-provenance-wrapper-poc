"""The invariant this whole harness exists to hold: no ground truth reaches a detection decision."""

import inspect

import numpy as np
import pytest

from genomark_n.payload import MessageCodec, crc24, false_accept_bound
from genomark_n.pipeline import Detector


def test_detect_has_no_payload_parameter():
    parameters = set(inspect.signature(Detector.detect).parameters)
    assert parameters == {"self", "audio", "thresholds"}


def test_flip_search_is_a_fixed_constant_independent_of_truth():
    codec = MessageCodec()
    assert codec.crc_trials == 11
    assert codec.patterns == (
        (), (0,), (1,), (2,), (3,), (0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)
    )
    rng = np.random.default_rng(0)
    logits = rng.normal(size=56)
    order_a = np.argsort(np.abs(logits), kind="stable")[:4]
    order_b = np.argsort(np.abs(-logits), kind="stable")[:4]
    assert np.array_equal(order_a, order_b)


def test_crc24_openpgp_check_value():
    assert crc24(b"123456789") == 0x21CF02


def test_two_bit_errors_recover_and_three_do_not():
    codec = MessageCodec()
    bits = codec.encode(codec.random_message(np.random.default_rng(1)))
    logits = (bits.astype(float) * 2.0 - 1.0) * 4.0
    weak = [3, 11, 29, 47]
    for index in weak:
        logits[index] = np.sign(logits[index]) * 0.1
    two = logits.copy()
    two[weak[0]] *= -1
    two[weak[1]] *= -1
    decoded = codec.decode(two)
    assert decoded is not None
    assert decoded.bits_corrected == 2
    assert np.array_equal(decoded.bits, bits)

    three = two.copy()
    three[weak[2]] *= -1
    assert codec.decode(three) is None


def test_false_accept_budget_matches_spec_3_4():
    codec = MessageCodec()
    assert false_accept_bound(64, codec) == pytest.approx(4.2e-5, rel=0.02)
