#!/usr/bin/env python3
"""Validate the machine-readable proxy capability coverage contract."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
P0_LAYERS = {"unit", "integration", "e2e"}
P0_PLATFORMS = {"linux", "macos", "windows"}


def validate(document: dict, repo_root: Path) -> list[str]:
    errors: list[str] = []
    if document.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    capabilities = document.get("capabilities")
    if not isinstance(capabilities, list) or not capabilities:
        return errors + ["at least one [[capabilities]] entry is required"]

    seen: set[str] = set()
    for index, capability in enumerate(capabilities, start=1):
        prefix = f"capability[{index}]"
        capability_id = capability.get("id")
        if not isinstance(capability_id, str) or not capability_id:
            errors.append(f"{prefix}: id is required")
            continue
        prefix = capability_id
        if capability_id in seen:
            errors.append(f"{prefix}: duplicate id")
        seen.add(capability_id)

        criticality = capability.get("criticality")
        if criticality not in {"p0", "p1", "p2"}:
            errors.append(f"{prefix}: criticality must be p0, p1, or p2")
        if not capability.get("owner"):
            errors.append(f"{prefix}: owner is required")

        layers = set(capability.get("layers", []))
        platforms = set(capability.get("platforms", []))
        failure_modes = capability.get("failure_modes", [])
        evidence = capability.get("evidence", [])
        if criticality == "p0" and not P0_LAYERS.issubset(layers):
            errors.append(f"{prefix}: p0 requires unit, integration, and e2e layers")
        if criticality == "p0" and not P0_PLATFORMS.issubset(platforms):
            errors.append(f"{prefix}: p0 requires linux, macos, and windows")
        if not failure_modes:
            errors.append(f"{prefix}: at least one failure mode is required")
        if len(evidence) < 2:
            errors.append(f"{prefix}: at least two evidence files are required")
        for path in evidence:
            if not isinstance(path, str) or not (repo_root / path).is_file():
                errors.append(f"{prefix}: missing evidence file: {path}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", nargs="?", default="e2e-tests/capabilities.json")
    args = parser.parse_args()
    path = REPO_ROOT / args.manifest
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: cannot read capability manifest: {exc}", file=sys.stderr)
        return 2
    errors = validate(document, REPO_ROOT)
    if errors:
        print("E2E capability contract: FAIL")
        for error in errors:
            print(f"  - {error}")
        return 1
    print(f"E2E capability contract: PASS ({len(document['capabilities'])} capabilities)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
