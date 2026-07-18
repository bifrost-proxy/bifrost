#!/usr/bin/env python3
"""Self-contained MLX runner for Bifrost MOSS joint transcription."""

from __future__ import annotations

import argparse
import json
import math
import os
from pathlib import Path
import re
import sys
from typing import Any


DEFAULT_PROTOCOL_PROMPT = (
    "请将音频转写为文本，每一段需以起始时间戳和说话人编号"
    "（[S01]、[S02]、[S03]…）开头，正文为对应的语音内容，"
    "并在段末标注结束时间戳，以清晰标明该段语音范围。"
)
SPEAKER_PREFIX_RE = re.compile(r"^\[(S\d+)\]\s*")


def compose_prompt(user_prompt: str) -> str:
    context = user_prompt.strip()
    if not context:
        return DEFAULT_PROTOCOL_PROMPT
    return (
        f"{DEFAULT_PROTOCOL_PROMPT}\n"
        "补充转写提示（不得改变上述时间戳和说话人格式）："
        f"{context}"
    )


def normalize_segments(raw_segments: list[dict[str, Any]]) -> list[dict[str, Any]]:
    segments: list[dict[str, Any]] = []
    for raw in raw_segments:
        try:
            start = float(raw["start"])
            end = float(raw["end"])
        except (KeyError, TypeError, ValueError):
            continue
        if not math.isfinite(start) or not math.isfinite(end) or end < start:
            continue

        text = str(raw.get("text") or "").strip()
        speaker = str(raw.get("speaker_id") or raw.get("speaker") or "").strip()
        prefix = SPEAKER_PREFIX_RE.match(text)
        if prefix:
            if not speaker:
                speaker = prefix.group(1)
            text = text[prefix.end() :].strip()
        if not text or not speaker:
            continue
        segments.append(
            {
                "start": start,
                "end": end,
                "speaker": speaker,
                "text": text,
            }
        )
    return segments


def read_prompt(path: str | None) -> str:
    if not path:
        return ""
    return Path(path).read_text(encoding="utf-8")


def self_test() -> int:
    import mlx.core as mx
    from mlx_audio.stt import load as mlx_audio_load

    assert mx.array([1], dtype=mx.int32).item() == 1
    assert callable(mlx_audio_load)
    prompt = compose_prompt("Bifrost、NextOnCall")
    assert prompt.startswith(DEFAULT_PROTOCOL_PROMPT)
    assert prompt.endswith("Bifrost、NextOnCall")
    segments = normalize_segments(
        [
            {
                "start": 0.1,
                "end": 1.2,
                "speaker_id": "S01",
                "text": "[S01] 测试",
            }
        ]
    )
    assert segments == [
        {"start": 0.1, "end": 1.2, "speaker": "S01", "text": "测试"}
    ]
    print("moss-mlx-runtime ok")
    return 0


def transcribe(args: argparse.Namespace) -> int:
    model_dir = Path(args.model_dir)
    audio = Path(args.audio)
    if not model_dir.is_dir():
        raise FileNotFoundError(f"MOSS model directory not found: {model_dir}")
    if not audio.is_file():
        raise FileNotFoundError(f"audio file not found: {audio}")

    # The release contains a pinned local snapshot. Never fall through to a
    # user cache or execute a network refresh during inference.
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    os.environ["PYTHONNOUSERSITE"] = "1"

    from mlx_audio.stt import load

    model = load(model_dir)
    result = model.generate(
        str(audio),
        max_tokens=args.max_new,
        temperature=0.0,
        prompt=compose_prompt(read_prompt(args.prompt_file)),
        prefill_step_size=2048,
        verbose=False,
    )
    segments = normalize_segments(list(result.segments or []))
    if not segments:
        raise RuntimeError("MOSS MLX returned no valid speaker-aware segments")
    json.dump(segments, sys.stdout, ensure_ascii=False, separators=(",", ":"))
    sys.stdout.write("\n")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="moss-mlx-runner")
    parser.add_argument("--self-test", action="store_true")
    subcommands = parser.add_subparsers(dest="command")
    command = subcommands.add_parser("transcribe")
    command.add_argument("model_dir")
    command.add_argument("audio")
    command.add_argument("--max-new", type=int, required=True)
    command.add_argument("--format", choices=("json",), default="json")
    command.add_argument("--prompt-file")
    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if args.command == "transcribe":
        return transcribe(args)
    parser.print_help(sys.stderr)
    return 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:  # noqa: BLE001 - CLI boundary must report cleanly.
        print(f"moss-mlx-runtime error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
