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
TRANSCRIPT_SEGMENT_RE = re.compile(
    r"\[(?P<start>\d+(?:\.\d+)?)\]\[(?P<speaker>S\d+)\]"
    r"(?P<text>.*?)\[(?P<end>\d+(?:\.\d+)?)\]",
    re.DOTALL,
)
EARLY_PROTOCOL_TOKEN_LIMIT = 256


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
        if not math.isfinite(start) or not math.isfinite(end) or end <= start:
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


def transcript_is_degenerate(segments: list[dict[str, Any]]) -> bool:
    """Reject obvious autoregressive loops without filtering normal speech."""
    texts = [str(segment.get("text") or "").strip() for segment in segments]
    combined = "".join(texts)
    non_space = [character for character in combined if not character.isspace()]
    if len(non_space) >= 100 and len(set(non_space)) <= 2:
        return True
    if len(texts) >= 20:
        counts: dict[str, int] = {}
        for text in texts:
            counts[text] = counts.get(text, 0) + 1
        if max(counts.values(), default=0) / len(texts) >= 0.9:
            return True
    return False


def parse_protocol_segments(text: str) -> list[dict[str, Any]]:
    """Parse the exact timestamp/speaker protocol required by Bifrost."""
    return normalize_segments(
        [
            {
                "start": match.group("start"),
                "end": match.group("end"),
                "speaker_id": match.group("speaker"),
                "text": match.group("text"),
            }
            for match in TRANSCRIPT_SEGMENT_RE.finditer(text)
        ]
    )


def protocol_output_has_complete_segment(text: str) -> bool:
    """Return whether generation produced one complete speaker-aware segment."""
    return bool(parse_protocol_segments(text))


def generate_protocol_segments(
    model: Any,
    audio: Path,
    *,
    max_tokens: int,
    prompt: str,
) -> tuple[list[dict[str, Any]], str]:
    """Generate with an early guard against long malformed/no-speech output.

    A valid MOSS response must produce a complete positive-duration
    ``[start][Sxx]text[end]`` segment. Waiting for the complete autoregressive
    budget when that contract never appears can waste several minutes on
    sparse/noisy recordings. The pinned MLX model exposes
    ``stream_generate``; collecting its token ids preserves the same final
    decode as ``generate`` while allowing a bounded protocol check.
    """
    from mlx_lm.sample_utils import make_sampler

    generated_tokens: list[int] = []
    first_segment_complete = False
    for token, _ in model.stream_generate(
        str(audio),
        max_tokens=max_tokens,
        sampler=make_sampler(0.0),
        prompt=prompt,
        prefill_step_size=2048,
        verbose=False,
    ):
        generated_tokens.append(int(token))
        if first_segment_complete:
            continue
        token_count = len(generated_tokens)
        if token_count % 16 != 0 and token_count < EARLY_PROTOCOL_TOKEN_LIMIT:
            continue
        tokenizer = getattr(model, "_tokenizer", None)
        if tokenizer is None:
            raise RuntimeError("MOSS MLX tokenizer is unavailable during generation")
        prefix = tokenizer.decode(generated_tokens, skip_special_tokens=True)
        first_segment_complete = protocol_output_has_complete_segment(prefix)
        if token_count >= EARLY_PROTOCOL_TOKEN_LIMIT and not first_segment_complete:
            raise RuntimeError(
                "MOSS output has no complete speaker-aware segment before "
                f"{EARLY_PROTOCOL_TOKEN_LIMIT} generated tokens"
            )

    tokenizer = getattr(model, "_tokenizer", None)
    if tokenizer is None:
        raise RuntimeError("MOSS MLX tokenizer is unavailable after generation")
    text = tokenizer.decode(generated_tokens, skip_special_tokens=True).strip()
    segments = parse_protocol_segments(text)
    if not segments:
        raise RuntimeError("MOSS MLX returned no valid speaker-aware segments")
    if transcript_is_degenerate(segments):
        raise RuntimeError("MOSS MLX returned degenerate repetitive transcription")
    finish_reason = "length" if len(generated_tokens) >= max_tokens else "completed"
    return segments, finish_reason


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
    assert protocol_output_has_complete_segment("[0.10][S01]测试[1.20]")
    assert not protocol_output_has_complete_segment("[0.10][S01]尚未结束")
    assert not protocol_output_has_complete_segment("[S01] 测试")
    assert parse_protocol_segments("[0.10][S01]测试[1.20]") == [
        {"start": 0.1, "end": 1.2, "speaker": "S01", "text": "测试"}
    ]
    assert parse_protocol_segments("[0.10][S01]零时长[0.10]") == []
    assert transcript_is_degenerate(
        [{"text": "嗯" * 100, "start": 0.0, "end": 1.0, "speaker": "S01"}]
    )
    assert not transcript_is_degenerate(
        [{"text": "正常的会议转录内容", "start": 0.0, "end": 1.0, "speaker": "S01"}]
    )
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
    segments, finish_reason = generate_protocol_segments(
        model,
        audio,
        max_tokens=args.max_new,
        prompt=compose_prompt(read_prompt(args.prompt_file)),
    )
    json.dump(
        {"segments": segments, "finish_reason": finish_reason},
        sys.stdout,
        ensure_ascii=False,
        separators=(",", ":"),
    )
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
