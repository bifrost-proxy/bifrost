#!/usr/bin/env python3
"""Minimal ffmpeg stand-in for the assisted voiceprint shell E2E.

Coverage runners do not always install ffmpeg. This helper supports only the
PCM16 segment-cut invocation used by that test, while preserving its invalid
source failure assertion. Production code and environments continue to use the
real ffmpeg binary.
"""

from __future__ import annotations

import math
import struct
import sys
from pathlib import Path


def option_value(arguments: list[str], option: str) -> str:
    try:
        return arguments[arguments.index(option) + 1]
    except (ValueError, IndexError) as error:
        raise ValueError(f"missing required option: {option}") from error


def main() -> int:
    arguments = sys.argv[1:]
    try:
        source = Path(option_value(arguments, "-i"))
        duration_seconds = float(option_value(arguments, "-t"))
        sample_rate = int(option_value(arguments, "-ar"))
        output = Path(arguments[-1])
    except (ValueError, IndexError) as error:
        print(f"fake ffmpeg voiceprint: {error}", file=sys.stderr)
        return 2

    try:
        if source.read_bytes()[:4] != b"RIFF":
            raise ValueError("input is not a WAV file")
        frame_count = max(1, round(duration_seconds * sample_rate))
        with output.open("wb") as pcm:
            for index in range(frame_count):
                sample = int(12000 * math.sin(2 * math.pi * 440 * index / sample_rate))
                pcm.write(struct.pack("<h", sample))
    except (OSError, ValueError) as error:
        print(f"fake ffmpeg voiceprint: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
