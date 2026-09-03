"""Emit the two deterministic ONNX fixture graphs the genomark-n plumbing test runs.

These are NOT GenoMark-N. They carry no learned parameters and encode nothing. They exist so the
Rust inference path -- card load, hash check, shape check, session build, tensor marshalling,
dynamic time dimension, output extraction -- is executed against a real ONNX graph before any
trained model exists. Every output value is computable by hand from the input, so the Rust test
asserts numbers rather than "it ran".

    decoder(log_mag[1,1,320,T])
        presence_logit[1,T] = mean over the 320 band rows
        message_logit[1,56] = (mean over every element) * COEFF[i]

    encoder(log_mag[1,1,320,T], message[1,56])
        gain_raw[1,1,320,T] = tanh(log_mag * mean(message))

Run:  uv run --with onnx python tools/make_fixture_models.py
"""

import hashlib
import json
import pathlib

import onnx
from onnx import TensorProto, helper, numpy_helper

import numpy as np

OPSET = 17
BAND_BINS = 320
MESSAGE_BITS = 56
COEFF = [(i - 27.5) / 8.0 for i in range(MESSAGE_BITS)]

HERE = pathlib.Path(__file__).resolve().parent
FIXTURES = HERE.parent / "fixtures"


def _reduce_mean(name, inp, out, axes, keepdims):
    return helper.make_node("ReduceMean", [inp], [out], name=name, axes=axes, keepdims=keepdims)


def decoder_model():
    log_mag = helper.make_tensor_value_info("log_mag", TensorProto.FLOAT, [1, 1, BAND_BINS, "T"])
    presence = helper.make_tensor_value_info("presence_logit", TensorProto.FLOAT, [1, "T"])
    message = helper.make_tensor_value_info("message_logit", TensorProto.FLOAT, [1, MESSAGE_BITS])

    coeff = numpy_helper.from_array(
        np.array([COEFF], dtype=np.float32).reshape(1, MESSAGE_BITS), name="coeff"
    )
    nodes = [
        _reduce_mean("band_mean", "log_mag", "presence_logit", [1, 2], 0),
        _reduce_mean("global_mean", "presence_logit", "global_mean_out", [0, 1], 1),
        helper.make_node("Mul", ["global_mean_out", "coeff"], ["message_logit"], name="scale"),
    ]
    graph = helper.make_graph(
        nodes, "genomark_n_fixture_decoder", [log_mag], [presence, message], initializer=[coeff]
    )
    return helper.make_model(
        graph, opset_imports=[helper.make_opsetid("", OPSET)], ir_version=9
    )


def encoder_model():
    log_mag = helper.make_tensor_value_info("log_mag", TensorProto.FLOAT, [1, 1, BAND_BINS, "T"])
    message = helper.make_tensor_value_info("message", TensorProto.FLOAT, [1, MESSAGE_BITS])
    gain = helper.make_tensor_value_info("gain_raw", TensorProto.FLOAT, [1, 1, BAND_BINS, "T"])

    nodes = [
        _reduce_mean("message_mean", "message", "message_mean_out", [0, 1], 1),
        helper.make_node("Mul", ["log_mag", "message_mean_out"], ["scaled"], name="scale"),
        helper.make_node("Tanh", ["scaled"], ["gain_raw"], name="squash"),
    ]
    graph = helper.make_graph(
        nodes, "genomark_n_fixture_encoder", [log_mag, message], [gain]
    )
    return helper.make_model(
        graph, opset_imports=[helper.make_opsetid("", OPSET)], ir_version=9
    )


def write(model, path):
    onnx.checker.check_model(model, full_check=True)
    data = model.SerializeToString()
    path.write_bytes(data)
    return hashlib.sha256(data).hexdigest()


def main():
    FIXTURES.mkdir(exist_ok=True)
    decoder_sha = write(decoder_model(), FIXTURES / "fixture-decoder-v1.onnx")
    encoder_sha = write(encoder_model(), FIXTURES / "fixture-encoder-v1.onnx")

    card_path = FIXTURES / "genomark-n-fixture-v1.card.json"
    card = json.loads(card_path.read_text())
    card["graphs"]["decoder"]["sha256"] = decoder_sha
    card["graphs"]["encoder"]["sha256"] = encoder_sha
    card_path.write_text(json.dumps(card, indent=2, sort_keys=False) + "\n")

    print(f"decoder sha256 {decoder_sha}")
    print(f"encoder sha256 {encoder_sha}")
    print(f"message coefficients {COEFF[:3]} ... {COEFF[-1]}")


if __name__ == "__main__":
    main()
