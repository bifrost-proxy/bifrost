#!/usr/bin/env python3
"""Transcribe system-generated speech with the actual extracted MOSS runtime."""

from pathlib import Path
import array
import hashlib
import json
import math
import os
import re
import subprocess
import sys
import wave


runtime = Path(sys.argv[1]).resolve()
source = Path(sys.argv[2]).read_text()
work = runtime.parent


def constant(name: str, pattern: str) -> str:
    match = re.search(rf"const {name}: [^=]+ =\s*{pattern};", source)
    assert match is not None, f"missing pinned {name}"
    return match.group(1)


url = constant("MOSS_MODEL_URL", r'"([^"]+)"')
expected_sha = constant("MOSS_MODEL_SHA256", r'"([0-9a-f]{64})"')
expected_bytes = int(constant("MOSS_MODEL_BYTES", r"([0-9_]+)").replace("_", ""))
assert re.fullmatch(
    r"https://huggingface.co/majentik/MOSS-Transcribe-Diarize-MLX-8bit/resolve/[0-9a-f]{40}/model.safetensors",
    url,
), "model must use the existing pinned public snapshot"
model = runtime / "model/model.safetensors"
subprocess.run(
    ["curl", "--fail", "--location", "--retry", "3", "--output", str(model), url],
    check=True,
    timeout=600,
)
assert model.stat().st_size == expected_bytes, "model byte length mismatch"
digest = hashlib.sha256()
with model.open("rb") as stream:
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(block)
assert digest.hexdigest() == expected_sha, "model checksum mismatch"
print(f"PASS: official pinned model verified ({expected_bytes} bytes, SHA256 {expected_sha})", flush=True)

# Public, synthetic speech only. Do not provide the expected words as a prompt.
speech = (
    "The meeting starts at nine tomorrow morning. "
    "Please bring the project notes and the updated schedule. "
    "We will review the results together and discuss the next steps."
)
subprocess.run(["/usr/bin/say", "-v", "Samantha", "-o", str(work / "speech.aiff"), speech], check=True, timeout=60)
audio = work / "speech.wav"
subprocess.run(
    ["/usr/bin/afconvert", "-f", "WAVE", "-d", "LEI16@16000", str(work / "speech.aiff"), str(audio)],
    check=True,
    timeout=60,
)
with wave.open(str(audio)) as wav:
    assert wav.getsampwidth() == 2 and wav.getframerate() == 16000
    duration = wav.getnframes() / wav.getframerate()
    samples = array.array("h", wav.readframes(wav.getnframes()))
assert duration > 5 and samples, "speech fixture is too short"
assert max(abs(sample) for sample in samples) > 100, "speech fixture is silent"

env = dict(os.environ, HF_HUB_OFFLINE="1", TRANSFORMERS_OFFLINE="1")
# Block Python network connections during inference, in addition to offline flags.
offline_entry = """
import runpy, socket, sys
def reject_network(*args, **kwargs):
    raise RuntimeError('network connection attempted during offline transcription')
socket.socket.connect = reject_network
socket.socket.connect_ex = reject_network
socket.create_connection = reject_network
runner = sys.argv.pop(1)
runpy.run_path(runner, run_name='__main__')
"""
result = subprocess.run(
    [sys.executable, "-c", offline_entry, str(runtime / "runtime/moss_mlx_runner.py"),
     "transcribe", str(runtime / "model"), str(audio), "--max-new", "1024"],
    env=env, text=True, capture_output=True, timeout=600,
)
print(result.stderr, file=sys.stderr, end="")
print(result.stdout, end="", flush=True)
assert result.returncode == 0, f"offline transcription exited {result.returncode}"
transcript = json.loads(result.stdout)
assert transcript["finish_reason"] == "completed", "transcription exhausted its token budget"
segments = transcript["segments"]
assert segments, "transcription contains no segments"
for segment in segments:
    assert re.fullmatch(r"S\d+", segment["speaker"])
    start, end = segment["start"], segment["end"]
    assert math.isfinite(start) and math.isfinite(end) and 0 <= start < end <= duration + 2
words = set(re.findall(r"[a-z]+", " ".join(s["text"] for s in segments).lower()))
matched = words & {"meeting", "tomorrow", "morning", "project", "schedule", "results"}
assert len(matched) >= 4, f"speech content mismatch; matched keywords: {sorted(matched)}"
print(f"PASS: real offline transcription of {duration:.2f}s synthetic speech; {len(segments)} timed speaker segments; matched {sorted(matched)}")
