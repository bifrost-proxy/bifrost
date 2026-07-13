#!/usr/bin/env python3
"""Quarantine incomplete LLVM raw profiles before coverage report generation."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from pathlib import Path


def sanitize(profile_root: Path, llvm_profdata: str) -> dict[str, object]:
    profiles = sorted(profile_root.rglob("*.profraw"))
    quarantine = profile_root / "corrupt"
    invalid: list[str] = []
    for profile in profiles:
        if quarantine in profile.parents:
            continue
        result = subprocess.run(
            [llvm_profdata, "show", str(profile)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
        )
        if result.returncode == 0:
            continue
        quarantine.mkdir(parents=True, exist_ok=True)
        destination = quarantine / profile.name
        counter = 1
        while destination.exists():
            destination = quarantine / f"{profile.stem}-{counter}{profile.suffix}"
            counter += 1
        shutil.move(str(profile), destination)
        invalid.append(str(profile.relative_to(profile_root)))
    return {
        "checked": len(profiles),
        "quarantined": len(invalid),
        "invalid_profiles": invalid,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("profile_root", type=Path)
    parser.add_argument("--llvm-profdata", required=True)
    parser.add_argument("--json-output", type=Path)
    args = parser.parse_args()
    result = sanitize(args.profile_root, args.llvm_profdata)
    if args.json_output:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(
        f"Validated {result['checked']} LLVM profiles; "
        f"quarantined {result['quarantined']} incomplete profile(s)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
