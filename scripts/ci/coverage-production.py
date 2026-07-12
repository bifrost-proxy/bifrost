#!/usr/bin/env python3
"""Report production Rust coverage with exact ``cfg(test)`` items excluded."""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from collections import defaultdict
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
CRATE_SOURCE_RE = re.compile(r"^crates/([^/]+)/src/.+\.rs$")
EXTERNAL_TEST_MODULE_RE = re.compile(
    r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;",
    re.MULTILINE,
)


def normalize_path(path: str, repo_root: Path = REPO_ROOT) -> str:
    normalized = path.replace("\\", "/")
    root = str(repo_root.resolve()).replace("\\", "/").rstrip("/")
    if normalized.startswith(root + "/"):
        return normalized[len(root) + 1 :]
    marker = "/crates/"
    if marker in normalized:
        return "crates/" + normalized.rsplit(marker, 1)[1]
    return normalized.removeprefix("./")


def parse_lcov(text: str, repo_root: Path = REPO_ROOT) -> dict[str, dict[int, int]]:
    coverage: dict[str, dict[int, int]] = defaultdict(dict)
    current_file: str | None = None
    for raw in text.splitlines():
        if raw.startswith("SF:"):
            current_file = normalize_path(raw[3:], repo_root)
        elif raw.startswith("DA:") and current_file is not None:
            line_no, hits, *_ = raw[3:].split(",")
            number = int(line_no)
            coverage[current_file][number] = max(
                coverage[current_file].get(number, 0), int(hits)
            )
        elif raw == "end_of_record":
            current_file = None
    return dict(coverage)


def rust_test_module_lines(source: str) -> set[int]:
    """Return source lines belonging to items annotated with exact ``cfg(test)``.

    The name is retained for callers, but this handles modules, functions,
    imports, constants, and other test-only items. Coverage builds compile all
    of them; counting those lines as production would make the denominator
    depend on the amount of test support code in a source file.
    """
    excluded: set[int] = set()
    pending_cfg_line: int | None = None
    in_item = False
    depth = 0
    opened = False
    for line_no, line in enumerate(source.splitlines(), start=1):
        stripped = line.strip()
        if not in_item:
            if stripped == "#[cfg(test)]":
                pending_cfg_line = line_no
                excluded.add(line_no)
                continue
            if pending_cfg_line is not None:
                excluded.add(line_no)
                if not stripped or stripped.startswith("#") or stripped.startswith("//"):
                    continue
                in_item = True
                opened = "{" in line
                depth = line.count("{") - line.count("}")
                if (opened and depth <= 0) or (not opened and ";" in line):
                    in_item = False
                    pending_cfg_line = None
                continue
            continue

        excluded.add(line_no)
        depth += line.count("{") - line.count("}")
        opened = opened or "{" in line
        if (opened and depth <= 0) or (not opened and ";" in line):
            in_item = False
            pending_cfg_line = None
    return excluded


def external_test_module_paths(source_path: Path, source: str) -> set[Path]:
    """Resolve files referenced by exact ``#[cfg(test)] mod name;`` items."""
    if source_path.stem in {"lib", "main", "mod"}:
        module_dir = source_path.parent
    else:
        module_dir = source_path.parent / source_path.stem

    resolved: set[Path] = set()
    for module_name in EXTERNAL_TEST_MODULE_RE.findall(source):
        candidates = (
            module_dir / f"{module_name}.rs",
            module_dir / module_name / "mod.rs",
        )
        resolved.update(path.resolve() for path in candidates if path.is_file())
    return resolved


def evaluate(
    coverage: dict[str, dict[int, int]],
    repo_root: Path = REPO_ROOT,
    crate_filter: str | None = None,
) -> dict[str, object]:
    excluded_external_modules: set[Path] = set()
    crates_root = repo_root / "crates"
    if crates_root.is_dir():
        for source_path in crates_root.glob("*/src/**/*.rs"):
            relative = normalize_path(str(source_path), repo_root)
            match = CRATE_SOURCE_RE.match(relative)
            if match is None or (crate_filter is not None and match.group(1) != crate_filter):
                continue
            excluded_external_modules.update(
                external_test_module_paths(
                    source_path, source_path.read_text(encoding="utf-8")
                )
            )

    crates: dict[str, dict[str, object]] = {}
    for path, lines in sorted(coverage.items()):
        match = CRATE_SOURCE_RE.match(path)
        if not match:
            continue
        crate = match.group(1)
        if crate_filter is not None and crate != crate_filter:
            continue
        source_path = repo_root / path
        if not source_path.is_file():
            continue
        if source_path.resolve() in excluded_external_modules:
            continue
        excluded = rust_test_module_lines(source_path.read_text(encoding="utf-8"))
        production = {line: hits for line, hits in lines.items() if line not in excluded}
        covered = sum(hits > 0 for hits in production.values())
        total = len(production)
        entry = crates.setdefault(crate, {"covered": 0, "total": 0, "files": []})
        entry["covered"] = int(entry["covered"]) + covered
        entry["total"] = int(entry["total"]) + total
        entry["files"].append(
            {
                "file": path,
                "covered": covered,
                "total": total,
                "missed": total - covered,
                "percent": 100.0 if total == 0 else covered * 100.0 / total,
            }
        )

    workspace_covered = 0
    workspace_total = 0
    for entry in crates.values():
        covered = int(entry["covered"])
        total = int(entry["total"])
        entry["percent"] = 100.0 if total == 0 else covered * 100.0 / total
        entry["files"] = sorted(
            entry["files"], key=lambda item: (-int(item["missed"]), str(item["file"]))
        )
        workspace_covered += covered
        workspace_total += total
    return {
        "schema_version": 1,
        "workspace": {
            "covered": workspace_covered,
            "total": workspace_total,
            "percent": (
                100.0
                if workspace_total == 0
                else workspace_covered * 100.0 / workspace_total
            ),
        },
        "crates": crates,
    }


def render(result: dict[str, object], threshold: float, top: int) -> str:
    lines = ["Production Rust coverage (all exact #[cfg(test)] items excluded)", ""]
    crates = result["crates"]
    assert isinstance(crates, dict)
    for crate, raw_entry in sorted(crates.items()):
        entry = raw_entry
        assert isinstance(entry, dict)
        percent = float(entry["percent"])
        covered = int(entry["covered"])
        total = int(entry["total"])
        target = math.ceil(total * threshold / 100.0)
        missing = max(0, target - covered)
        status = "PASS" if percent + 1e-9 >= threshold else "FAIL"
        lines.append(
            f"{crate}: {percent:.2f}% ({covered}/{total}), "
            f"need {missing} lines for {threshold:g}% [{status}]"
        )
        files = entry["files"]
        assert isinstance(files, list)
        for file_entry in files[:top]:
            lines.append(
                f"  {int(file_entry['missed']):5d} missed  "
                f"{float(file_entry['percent']):6.2f}%  {file_entry['file']}"
            )
    return "\n".join(lines)


def threshold_from_manifest(path: Path, crate: str) -> float:
    """Read a crate floor without requiring Python 3.11's tomllib."""
    section = ""
    default: float | None = None
    crate_floor: float | None = None
    wanted = f"crates.{crate}"
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1]
            continue
        if "=" not in line:
            continue
        key, value = (part.strip() for part in line.split("=", 1))
        if section == "settings" and key == "default":
            default = float(value)
        elif section == wanted and key == "min":
            crate_floor = float(value)
    if crate_floor is not None:
        return crate_floor
    if default is not None:
        return default
    raise ValueError(f"no floor for {crate!r} in {path}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("lcov", type=Path)
    parser.add_argument("--crate")
    parser.add_argument("--threshold", type=float)
    parser.add_argument(
        "--thresholds",
        type=Path,
        default=REPO_ROOT / "scripts/ci/coverage-thresholds.toml",
    )
    parser.add_argument("--top", type=int, default=20)
    parser.add_argument("--json-output", type=Path)
    parser.add_argument("--no-gate", action="store_true")
    args = parser.parse_args()

    threshold = args.threshold
    if threshold is None:
        if not args.crate:
            parser.error("--crate is required when --threshold is not supplied")
        threshold = threshold_from_manifest(args.thresholds, args.crate)

    coverage = parse_lcov(args.lcov.read_text(encoding="utf-8"))
    result = evaluate(coverage, crate_filter=args.crate)
    if args.json_output:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(render(result, threshold, args.top))

    crates = result["crates"]
    assert isinstance(crates, dict)
    if not crates:
        print("ERROR: no matching production Rust coverage records", file=sys.stderr)
        return 2
    if args.no_gate:
        return 0
    return int(
        any(
            float(entry["percent"]) + 1e-9 < threshold
            for entry in crates.values()
        )
    )


if __name__ == "__main__":
    raise SystemExit(main())
