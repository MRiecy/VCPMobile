#!/usr/bin/env python3
"""Compare original ONNX and mobile ORT over real speech and adversarial frames."""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import wave

import numpy as np
import onnxruntime as ort

from build_runtime import ROOT, MODEL, LOCK, prepare, sha


def run_stream(model: Path, audio: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    options = ort.SessionOptions()
    options.intra_op_num_threads = options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    options.graph_optimization_level = ort.GraphOptimizationLevel.ORT_DISABLE_ALL
    options.add_session_config_entry("session.intra_op.allow_spinning", "0")
    options.add_session_config_entry("session.inter_op.allow_spinning", "0")
    session = ort.InferenceSession(str(model), options, providers=["CPUExecutionProvider"])
    state, context = np.zeros((2, 1, 128), np.float32), np.zeros((1, 64), np.float32)
    probabilities, states = [], []
    for start in range(0, len(audio), 512):
        frame = np.zeros((1, 512), np.float32)
        part = audio[start:start + 512]
        frame[0, :len(part)] = part
        inputs = {"input": np.concatenate([context, frame], axis=1), "state": state,
                  "sr": np.array(16000, np.int64)}
        probability, state = session.run(["output", "stateN"], inputs)
        probabilities.append(float(probability.item()))
        states.append(state.copy())
        context = frame[:, -64:].copy()
    return np.array(probabilities), np.stack(states)


def endpoints(probabilities: np.ndarray) -> list[list[int | str]]:
    # A fixed consumer only for comparing the two formats, not a product contract.
    events, high, low, speaking = [], 0, 0, False
    for frame, probability in enumerate(probabilities):
        high = high + 1 if probability >= 0.7 else 0
        low = low + 1 if probability < 0.35 else 0
        if not speaking and high >= 3:
            speaking = True
            events.append([frame * 512, "start"])
        elif speaking and low >= 16:
            speaking = False
            events.append([frame * 512, "end"])
    return events


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache-dir", type=Path, required=True)
    args = parser.parse_args()
    if ort.__version__ != LOCK["ortVersion"]:
        raise RuntimeError("Use requirements-build.txt")
    original = prepare(args.cache_dir)
    probe = ROOT / "assets/voice/voice-probe.wav"
    with wave.open(str(probe)) as source:
        if (source.getnchannels(), source.getsampwidth(), source.getframerate()) != (1, 2, 16000):
            raise RuntimeError("The probe must be mono 16kHz PCM16")
        speech = np.frombuffer(source.readframes(source.getnframes()), dtype="<i2").astype(np.float32) / 32768
    rng = np.random.default_rng(20261019)
    cases = {
        "real-speech-with-silence": np.concatenate([np.zeros(16000), speech, np.zeros(32000)]).astype(np.float32),
        "silence-noise-clipping": np.concatenate([np.zeros(16000), rng.normal(0, .005, 16000),
            rng.uniform(-1, 1, 16000), np.ones(16000), np.zeros(16000)]).astype(np.float32),
        "quiet-speech": np.concatenate([np.zeros(16000), speech * .05, np.zeros(32000)]).astype(np.float32),
    }
    report = {"runtime": ort.__version__, "originalSha256": sha(original), "ortSha256": sha(MODEL),
              "probeSha256": sha(probe), "scope": "Host CPU verifies conversion equivalence, not mobile native execution", "cases": []}
    for name, audio in cases.items():
        expected, expected_states = run_stream(original, audio)
        actual, actual_states = run_stream(MODEL, audio)
        if not all(np.isfinite(values).all() for values in (expected, expected_states, actual, actual_states)):
            raise AssertionError(f"Non-finite probability/state for {name}")
        np.testing.assert_allclose(actual, expected, atol=2e-5, rtol=2e-5)
        np.testing.assert_allclose(actual_states, expected_states, atol=2e-4, rtol=2e-4)
        if endpoints(actual) != endpoints(expected):
            raise AssertionError(f"Speech endpoints changed for {name}")
        report["cases"].append({"name": name, "frames": len(actual),
            "maxProbabilityDelta": float(np.max(np.abs(actual - expected))),
            "maxStateDelta": float(np.max(np.abs(actual_states - expected_states))),
            "speechEvents": endpoints(actual)})
    if not report["cases"][0]["speechEvents"]:
        raise AssertionError("Real speech probe must exercise a detected speech region")
    (ROOT / "model-verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
