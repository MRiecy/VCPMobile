"""Reproduce the offline echo probe from attributed, pinned FSDD recordings."""
import hashlib
import io
import json
from pathlib import Path
import urllib.request
import wave

import numpy as np

ROOT = Path(__file__).resolve().parent
REVISION = "26eb9aaf76e81b692f806f9140c2d2777410d7a1"
BASE = f"https://raw.githubusercontent.com/Jakobovski/free-spoken-digit-dataset/{REVISION}"


def main():
    parts = [np.zeros(1600, dtype=np.float64)]
    sources = []
    # Fixed windowed-sinc interpolation, not sample repetition; original rate is 8k.
    taps = np.arange(-32, 33, dtype=np.float64)
    kernel = np.sinc(taps / 2) * np.hamming(len(taps))
    kernel *= 2 / kernel.sum()
    for digit in range(10):
        name = f"{digit}_jackson_0.wav"
        url = f"{BASE}/recordings/{name}"
        with urllib.request.urlopen(url, timeout=30) as response:
            data = response.read(128 * 1024)
        with wave.open(io.BytesIO(data), "rb") as recording:
            if (recording.getnchannels(), recording.getsampwidth(), recording.getframerate()) != (1, 2, 8000):
                raise RuntimeError(f"Unexpected source format: {name}")
            samples = np.frombuffer(recording.readframes(recording.getnframes()), dtype="<i2").astype(np.float64)
        interpolated = np.zeros(len(samples) * 2, dtype=np.float64)
        interpolated[::2] = samples
        interpolated = np.convolve(interpolated, kernel, mode="same")
        # Equal peak ceiling keeps this short diagnostic audible without clipping.
        peak = max(float(np.max(np.abs(interpolated))), 1)
        parts.extend([interpolated * (0.55 * 32767 / peak), np.zeros(1920, dtype=np.float64)])
        sources.append({"file": name, "url": url, "sha256": hashlib.sha256(data).hexdigest()})
    pcm = np.clip(np.rint(np.concatenate(parts)), -32768, 32767).astype("<i2")
    destination = ROOT / "assets" / "voice"
    destination.mkdir(parents=True, exist_ok=True)
    output = destination / "voice-probe.wav"
    with wave.open(str(output), "wb") as recording:
        recording.setparams((1, 2, 16000, 0, "NONE", "not compressed"))
        recording.writeframes(pcm.tobytes())
    metadata = {
        "title": "Local echo probe: spoken digits zero through nine",
        "attribution": "Free Spoken Digit Dataset (FSDD), Jakobovski and contributors; speaker jackson",
        "license": "CC-BY-SA-4.0",
        "license_url": "https://creativecommons.org/licenses/by-sa/4.0/",
        "source_license": f"{BASE}/README.md",
        "revision": REVISION,
        "changes": "Resampled 8kHz to 16kHz, peak-normalized, concatenated with brief pauses. This derived audio remains CC-BY-SA-4.0.",
        "sample_rate": 16000,
        "channels": 1,
        "sample_format": "PCM signed 16-bit little-endian",
        "frames": len(pcm),
        "duration_seconds": len(pcm) / 16000,
        "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
        "sources": sources,
    }
    (destination / "voice-probe.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({key: metadata[key] for key in ("frames", "duration_seconds", "sha256")}))


if __name__ == "__main__":
    main()
