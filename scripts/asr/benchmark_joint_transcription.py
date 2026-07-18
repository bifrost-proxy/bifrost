#!/usr/bin/env python3
"""Build a read-only joint-transcription benchmark plan from an ASR task."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Select real successful ASR task samples near target durations"
    )
    parser.add_argument("--task-dir", type=Path, required=True)
    parser.add_argument(
        "--target-seconds", type=int, nargs="+", default=[600, 1800]
    )
    parser.add_argument("--hash-inputs", action="store_true")
    parser.add_argument("--output", type=Path)
    return parser.parse_args()


def load_json(path: Path) -> Any:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def successful_candidates(files_path: Path) -> list[dict[str, Any]]:
    payload = load_json(files_path)
    records = payload.get("files", {})
    if isinstance(records, dict):
        records = list(records.values())
    if not isinstance(records, list):
        raise ValueError(f"invalid files collection in {files_path}")

    candidates = []
    for record in records:
        if not isinstance(record, dict) or record.get("status") != "success":
            continue
        duration_ms = record.get("media_duration_ms")
        source_path = record.get("source_path")
        timeline_path = record.get("output_timeline_path")
        if not isinstance(duration_ms, int) or duration_ms <= 0:
            continue
        if not source_path or not timeline_path:
            continue
        source = Path(source_path)
        timeline = Path(timeline_path)
        if not source.is_file() or not timeline.is_file():
            continue
        candidates.append(record)
    return candidates


def sample_report(
    record: dict[str, Any], target_seconds: int, hash_inputs: bool
) -> dict[str, Any]:
    source_path = Path(record["source_path"])
    timeline_path = Path(record["output_timeline_path"])
    timeline = load_json(timeline_path)
    segments = timeline.get("segments", [])
    speakers = {
        segment.get("speaker")
        for segment in segments
        if isinstance(segment, dict) and segment.get("speaker")
    }
    if not speakers:
        speakers = {
            speaker.get("id")
            for speaker in timeline.get("speakers", [])
            if isinstance(speaker, dict) and speaker.get("id")
        }
    speech_end_ms = max(
        (
            int(segment.get("audio_end_ms", 0))
            for segment in segments
            if isinstance(segment, dict)
        ),
        default=0,
    )
    media_duration_ms = int(record["media_duration_ms"])
    chunk_metrics = record.get("chunk_metrics") or []
    elapsed_ms = sum(
        int(metric.get("elapsed_ms", 0))
        for metric in chunk_metrics
        if isinstance(metric, dict) and metric.get("status") == "ok"
    )
    report = {
        "target_seconds": target_seconds,
        "source_path": str(source_path),
        "timeline_path": str(timeline_path),
        "media_duration_ms": media_duration_ms,
        "target_delta_ms": abs(media_duration_ms - target_seconds * 1000),
        "model": timeline.get("model"),
        "language": timeline.get("language"),
        "text_chars": int(record.get("text_chars") or 0),
        "segment_count": len(segments),
        "speaker_count": len(speakers),
        "speakers": sorted(speakers),
        "reference_speech_end_ms": speech_end_ms,
        "speech_end_to_media_ratio": round(
            speech_end_ms / media_duration_ms if media_duration_ms else 0.0, 6
        ),
        "reference_inference_elapsed_ms": elapsed_ms,
        "reference_rtf": round(
            elapsed_ms / media_duration_ms if media_duration_ms else 0.0, 6
        ),
        "completeness_basis": "existing_timeline_speech_end",
        "completeness_status": "reference_only",
    }
    if hash_inputs:
        report["source_sha256"] = sha256(source_path)
        report["timeline_sha256"] = sha256(timeline_path)
    return report


def build_report(args: argparse.Namespace) -> dict[str, Any]:
    task_dir = args.task_dir.expanduser().resolve()
    files_path = task_dir / "files.json"
    if not files_path.is_file():
        raise FileNotFoundError(f"missing task files index: {files_path}")
    candidates = successful_candidates(files_path)
    if not candidates:
        raise ValueError(f"no readable successful ASR samples in {files_path}")

    samples = []
    used_sources: set[str] = set()
    for target_seconds in args.target_seconds:
        if target_seconds <= 0:
            raise ValueError("target durations must be positive")
        ordered = sorted(
            candidates,
            key=lambda record: (
                abs(int(record["media_duration_ms"]) - target_seconds * 1000),
                str(record["source_path"]),
            ),
        )
        selected = next(
            (record for record in ordered if record["source_path"] not in used_sources),
            ordered[0],
        )
        used_sources.add(selected["source_path"])
        samples.append(sample_report(selected, target_seconds, args.hash_inputs))

    report = {
        "schema_version": 1,
        "mode": "read_only_baseline",
        "task_dir": str(task_dir),
        "files_index": str(files_path),
        "successful_candidate_count": len(candidates),
        "targets_seconds": args.target_seconds,
        "samples": samples,
        "notes": [
            "Source audio, task indexes, and timelines are never modified.",
            "Existing timeline endpoints are references, not completeness ground truth.",
            "This report is a reproducible baseline, not a WER or DER quality claim.",
        ],
    }
    if args.hash_inputs:
        report["files_index_sha256"] = sha256(files_path)
    return report


def validate_output_path(output: Path, report: dict[str, Any]) -> Path:
    output = output.expanduser().resolve()
    task_dir = Path(report["task_dir"])
    protected = {Path(report["files_index"]).resolve()}
    files_payload = load_json(Path(report["files_index"]))
    records = files_payload.get("files", {})
    if isinstance(records, dict):
        records = list(records.values())
    if not isinstance(records, list):
        records = []
    for record in records:
        if not isinstance(record, dict):
            continue
        for key, value in record.items():
            if key == "source_path" or (key.startswith("output_") and key.endswith("_path")):
                if isinstance(value, str) and value:
                    protected.add(Path(value).expanduser().resolve())
    if output == task_dir or output.is_relative_to(task_dir) or output in protected:
        raise ValueError(f"refusing to write benchmark output over ASR task input: {output}")
    return output


def main() -> int:
    args = parse_args()
    try:
        report = build_report(args)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        raise SystemExit(f"benchmark plan failed: {error}") from error
    encoded = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        try:
            output = validate_output_path(args.output, report)
        except ValueError as error:
            raise SystemExit(f"benchmark plan failed: {error}") from error
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
