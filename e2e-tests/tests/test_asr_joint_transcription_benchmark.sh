#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOOL="$REPO_ROOT/scripts/asr/benchmark_joint_transcription.py"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

TASK_DIR="$TMP_DIR/task"
DATA_DIR="$TMP_DIR/data"
mkdir -p "$TASK_DIR" "$DATA_DIR"

printf 'ten-minute-source' > "$DATA_DIR/ten.wav"
printf 'thirty-minute-source' > "$DATA_DIR/thirty.wav"
printf 'unselected-source' > "$DATA_DIR/unselected.wav"

python3 - "$TASK_DIR" "$DATA_DIR" <<'PY'
import json
import sys
from pathlib import Path

task_dir = Path(sys.argv[1])
data_dir = Path(sys.argv[2])

def timeline(name, duration_ms, speech_end_ms, speaker_count):
    path = data_dir / f"{name}.timeline.json"
    speakers = [
        {"id": f"speaker_{index:02d}", "display_name": f"Speaker {index}"}
        for index in range(speaker_count)
    ]
    segments = [
        {
            "index": index,
            "audio_start_ms": index * 1000,
            "audio_end_ms": speech_end_ms if index == speaker_count - 1 else (index + 1) * 1000,
            "speaker": f"speaker_{index:02d}",
            "overlap": False,
            "text": f"segment {index}",
        }
        for index in range(speaker_count)
    ]
    path.write_text(json.dumps({
        "task_id": "fixture-task",
        "task_name": "fixture",
        "source_path": str(data_dir / f"{name}.wav"),
        "media_duration_ms": duration_ms,
        "model": "Qwen3-ASR-0.6B",
        "language": "chinese",
        "speakers": speakers,
        "segments": segments,
    }), encoding="utf-8")
    return path

ten_timeline = timeline("ten", 617_000, 605_000, 2)
thirty_timeline = timeline("thirty", 1_800_000, 1_706_000, 4)
unselected_timeline = timeline("unselected", 60_000, 55_000, 1)
files = {
    "ten": {
        "source_path": str(data_dir / "ten.wav"),
        "media_duration_ms": 617_000,
        "status": "success",
        "output_timeline_path": str(ten_timeline),
        "text_chars": 100,
        "chunk_metrics": [{"status": "ok", "elapsed_ms": 6_170}],
    },
    "thirty": {
        "source_path": str(data_dir / "thirty.wav"),
        "media_duration_ms": 1_800_000,
        "status": "success",
        "output_timeline_path": str(thirty_timeline),
        "text_chars": 200,
        "chunk_metrics": [{"status": "ok", "elapsed_ms": 18_000}],
    },
    "unselected": {
        "source_path": str(data_dir / "unselected.wav"),
        "media_duration_ms": 60_000,
        "status": "success",
        "output_timeline_path": str(unselected_timeline),
        "text_chars": 20,
        "chunk_metrics": [{"status": "ok", "elapsed_ms": 600}],
    },
    "failed": {
        "source_path": str(data_dir / "failed.wav"),
        "media_duration_ms": 600_000,
        "status": "failed",
    },
}
(task_dir / "files.json").write_text(
    json.dumps({"version": 1, "files": files}), encoding="utf-8"
)
PY

BEFORE_HASH="$(shasum -a 256 "$TASK_DIR/files.json" "$DATA_DIR/ten.wav" "$DATA_DIR/thirty.wav")"
REPORT="$TMP_DIR/report.json"
python3 "$TOOL" \
  --task-dir "$TASK_DIR" \
  --target-seconds 600 1800 \
  --hash-inputs \
  --output "$REPORT"
AFTER_HASH="$(shasum -a 256 "$TASK_DIR/files.json" "$DATA_DIR/ten.wav" "$DATA_DIR/thirty.wav")"

test "$BEFORE_HASH" = "$AFTER_HASH"
python3 - "$REPORT" <<'PY'
import json
import sys
from pathlib import Path

report = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert report["mode"] == "read_only_baseline"
assert report["successful_candidate_count"] == 3
assert [sample["media_duration_ms"] for sample in report["samples"]] == [617_000, 1_800_000]
assert [sample["speaker_count"] for sample in report["samples"]] == [2, 4]
assert report["samples"][1]["speech_end_to_media_ratio"] < 0.95
assert report["samples"][1]["completeness_status"] == "reference_only"
assert report["samples"][0]["reference_rtf"] == 0.01
assert "source_sha256" in report["samples"][0]
PY

# The read-only contract must fail closed even when an operator accidentally
# points --output at the task index or one of the selected timelines.
if python3 "$TOOL" \
  --task-dir "$TASK_DIR" \
  --target-seconds 600 1800 \
  --output "$TASK_DIR/files.json" > "$TMP_DIR/overwrite-index.log" 2>&1; then
  echo "benchmark tool unexpectedly overwrote files.json" >&2
  exit 1
fi
grep -q "refusing to write benchmark output over ASR task input" "$TMP_DIR/overwrite-index.log"

if python3 "$TOOL" \
  --task-dir "$TASK_DIR" \
  --target-seconds 600 1800 \
  --output "$DATA_DIR/unselected.timeline.json" > "$TMP_DIR/overwrite-timeline.log" 2>&1; then
  echo "benchmark tool unexpectedly overwrote a timeline" >&2
  exit 1
fi
grep -q "refusing to write benchmark output over ASR task input" "$TMP_DIR/overwrite-timeline.log"

FINAL_HASH="$(shasum -a 256 "$TASK_DIR/files.json" "$DATA_DIR/ten.wav" "$DATA_DIR/thirty.wav")"
test "$BEFORE_HASH" = "$FINAL_HASH"

echo "PASS: ASR joint-transcription benchmark plan is deterministic and read-only"
