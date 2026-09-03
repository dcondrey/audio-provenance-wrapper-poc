"""Spec 4.2: synchronisation is ELIMINATED, not solved.

The whole design rests on the decoder being shift-equivariant on the time axis, so that bulk
acoustic delay and an arbitrary capture start are absorbed by pooling rather than by an offset
search. Nothing else in the harness would notice if that property were lost: a positional encoding,
a time-strided layer, a flatten, or a global time-axis reduction feeding a per-frame output would
all still train, still export, and still produce plausible numbers.

Two distinct properties are asserted, because pooled invariance and dense equivariance are not the
same claim:
  (a) the 56 POOLED bit logits are stable under a shift that is NOT a whole number of hops. Spec
      4.4(a) is explicit that real acoustic delay is not frame-aligned and the augmentation must not
      be either; a shift of a whole hop would pass even for a model keying on STFT frame phase.
  (b) the DENSE per-frame presence logits TRANSLATE by the frame offset under a whole-hop shift.
      That is spec 2.6(a)'s localisation claim, and it is what lets a partial mark be found.

TOLERANCES ARE MEASURED, NOT HOPED. GroupNorm normalises over the whole time axis, so a crop taken
at a different offset sees slightly different global statistics and equivariance is approximate
rather than exact. The observed deviation on an untrained model is ~2e-3 relative for (a) and ~2e-2
relative for (b); the bounds below sit a few times above that. The control assertion is what stops
the test passing vacuously: a crop of DIFFERENT content must deviate several times more than a
shifted crop of the same content.
"""

import numpy as np
import pytest
import torch

from genomark_n.config import load_config
from genomark_n.data.synthetic import SyntheticCorpus
from genomark_n.model import GenoMarkN
from genomark_n.payload import MessageCodec
from genomark_n.perceptual import PerceptualModel
from genomark_n.pipeline import Marker
from genomark_n.stft import SpectralFront

SUB_FRAME_SHIFT = 137
WHOLE_HOP_SHIFT = 5 * 512
POOLED_TOLERANCE = 0.01
DENSE_TOLERANCE = 0.05
EDGE_FRAMES = 20


@pytest.fixture(scope="module")
def harness():
    torch.manual_seed(7)
    config = load_config()
    front = SpectralFront(config.stft)
    model = GenoMarkN(config.encoder, config.decoder, config.payload, config.stft).eval()
    marker = Marker(model, front, PerceptualModel(config.perceptual, config.stft.sample_rate),
                    config.budget)
    codec = MessageCodec(config.payload)
    bits = torch.from_numpy(
        codec.encode(codec.random_message(np.random.default_rng(3)))
    ).float().unsqueeze(0)
    rate = config.stft.sample_rate
    cover = torch.from_numpy(
        SyntheticCorpus(items=1, seconds=10.0, sample_rate=rate, base_seed=5).load(0)
    ).unsqueeze(0)
    with torch.no_grad():
        marked = marker(cover, bits, torch.ones(1, front.frames_for(cover.shape[-1])))["audio"]
    return model, front, marked, rate


def _decode(model, front, marked, offset, samples):
    with torch.no_grad():
        return model.decoder(front.band_log_magnitude(front.stft(marked[:, offset : offset + samples])))


def test_pooled_bit_logits_survive_a_sub_frame_shift(harness):
    model, front, marked, rate = harness
    samples, offset = 6 * rate, rate
    _, reference, _ = _decode(model, front, marked, offset, samples)
    _, shifted, _ = _decode(model, front, marked, offset + SUB_FRAME_SHIFT, samples)
    _, elsewhere, _ = _decode(model, front, marked, offset + 2 * rate, samples)

    scale = float(reference.abs().max())
    shift_error = float((reference - shifted).abs().max()) / scale
    control_error = float((reference - elsewhere).abs().max()) / scale

    assert shift_error < POOLED_TOLERANCE, f"pooled bit logits moved {shift_error:.3e} under a shift"
    assert control_error > 5.0 * shift_error, (
        f"different content deviated {control_error:.3e}, only {control_error / shift_error:.1f}x "
        "the shift deviation: the tolerance is not discriminating and this test proves nothing"
    )


def test_dense_presence_translates_with_a_whole_hop_shift(harness):
    model, front, marked, rate = harness
    samples, offset = 6 * rate, rate
    hops = WHOLE_HOP_SHIFT // front.config.hop
    reference, _, _ = _decode(model, front, marked, offset, samples)
    shifted, _, _ = _decode(model, front, marked, offset + WHOLE_HOP_SHIFT, samples)

    frames = reference.shape[-1]
    scale = float(reference.abs().max())
    left = reference[0, EDGE_FRAMES + hops : frames - EDGE_FRAMES]
    error = float((left - shifted[0, EDGE_FRAMES : frames - EDGE_FRAMES - hops]).abs().max()) / scale
    control = float((left - shifted[0, EDGE_FRAMES + hops : frames - EDGE_FRAMES]).abs().max()) / scale

    assert error < DENSE_TOLERANCE, (
        f"per-frame presence did not translate: {error:.3e} relative deviation after aligning "
        f"{hops} frames. The localisation claim of spec 2.6(a) is false if this fails."
    )
    assert control > 5.0 * error, (
        f"comparing the same frame indices without translating deviated only {control:.3e}, "
        f"{control / error:.1f}x the aligned error: the presence trace carries no frame-localised "
        "structure, so translating it proves nothing"
    )
