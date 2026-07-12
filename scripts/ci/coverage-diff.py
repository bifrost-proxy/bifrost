#!/usr/bin/env python3
"""Enforce line coverage for changed production Rust lines.

The workspace ratchet prevents global regression, but a large crate can still
absorb untested new code.  This gate intersects added lines from ``git diff``
with instrumentable lines from LCOV and applies a stricter PR threshold.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
PRODUCTION_RUST_RE = re.compile(r"^crates/[^/]+/src/.+\.rs$")


def normalize_path(path: str, repo_root: Path = REPO_ROOT) -> str:
    normalized = path.replace("\\", "/")
    root = str(repo_root.resolve()).replace("\\", "/").rstrip("/")
    if normalized.startswith(root + "/"):
        return normalized[len(root) + 1 :]
    marker = "/crates/"
    if marker in normalized:
        return "crates/" + normalized.rsplit(marker, 1)[1]
    return normalized.removeprefix("./")


def parse_diff(diff_text: str) -> dict[str, set[int]]:
    changed: dict[str, set[int]] = defaultdict(set)
    current_file: str | None = None
    for line in diff_text.splitlines():
        if line.startswith("+++ "):
            value = line[4:].strip()
            current_file = None if value == "/dev/null" else value.removeprefix("b/")
            continue
        match = HUNK_RE.match(line)
        if not match or current_file is None:
            continue
        start = int(match.group(1))
        count = int(match.group(2) or "1")
        changed[current_file].update(range(start, start + count))
    return dict(changed)


def parse_lcov(lcov_text: str, repo_root: Path = REPO_ROOT) -> dict[str, dict[int, int]]:
    coverage: dict[str, dict[int, int]] = defaultdict(dict)
    current_file: str | None = None
    for raw in lcov_text.splitlines():
        if raw.startswith("SF:"):
            current_file = normalize_path(raw[3:], repo_root)
        elif raw.startswith("DA:") and current_file is not None:
            line_no, hits, *_ = raw[3:].split(",")
            coverage[current_file][int(line_no)] = int(hits)
        elif raw == "end_of_record":
            current_file = None
    return dict(coverage)


def rust_test_module_lines(source: str) -> set[int]:
    """Return lines inside ``#[cfg(test)] mod ... {}`` blocks.

    LCOV reports inline test modules because they live in ``src/*.rs``.  They
    must not inflate the changed-production-lines numerator or denominator.
    This deliberately recognizes module blocks rather than individual
    ``#[cfg(test)]`` helpers, which can still be production-test seams.
    """
    excluded: set[int] = set()
    lines = source.splitlines()
    pending_cfg_line: int | None = None
    in_module = False
    depth = 0
    opened = False
    for line_no, line in enumerate(lines, start=1):
        stripped = line.strip()
        if not in_module:
            if stripped == "#[cfg(test)]":
                pending_cfg_line = line_no
                continue
            if pending_cfg_line is not None and re.search(r"\bmod\s+\w+", stripped):
                in_module = True
                opened = "{" in line
                depth = line.count("{") - line.count("}")
                excluded.update(range(pending_cfg_line, line_no + 1))
                if opened and depth <= 0:
                    in_module = False
                    pending_cfg_line = None
                continue
            if stripped and not stripped.startswith("#"):
                pending_cfg_line = None
            continue

        excluded.add(line_no)
        depth += line.count("{") - line.count("}")
        opened = opened or "{" in line
        if opened and depth <= 0:
            in_module = False
            pending_cfg_line = None
    return excluded


def exclude_inline_test_modules(
    changed: dict[str, set[int]], repo_root: Path = REPO_ROOT
) -> dict[str, set[int]]:
    filtered: dict[str, set[int]] = {}
    for path, lines in changed.items():
        source_path = repo_root / path
        if not PRODUCTION_RUST_RE.match(path) or not source_path.is_file():
            filtered[path] = set(lines)
            continue
        excluded = rust_test_module_lines(source_path.read_text(encoding="utf-8"))
        filtered[path] = set(lines).difference(excluded)
    return filtered


def evaluate_changed_coverage(
    changed: dict[str, set[int]], coverage: dict[str, dict[int, int]]
) -> dict[str, object]:
    files: list[dict[str, object]] = []
    total = 0
    covered = 0
    for path in sorted(changed):
        if not PRODUCTION_RUST_RE.match(path):
            continue
        instrumented = coverage.get(path, {})
        relevant = sorted(changed[path].intersection(instrumented))
        if not relevant:
            continue
        missed = [line for line in relevant if instrumented[line] == 0]
        file_covered = len(relevant) - len(missed)
        total += len(relevant)
        covered += file_covered
        files.append(
            {
                "file": path,
                "covered": file_covered,
                "total": len(relevant),
                "missed_lines": missed,
            }
        )
    percent = 100.0 if total == 0 else covered * 100.0 / total
    return {"covered": covered, "total": total, "percent": percent, "files": files}


def git_diff(base_ref: str) -> str:
    command = [
        "git",
        "diff",
        "--unified=0",
        "--diff-filter=AM",
        f"{base_ref}...HEAD",
        "--",
        "crates/*/src/*.rs",
        "crates/*/src/**/*.rs",
    ]
    result = subprocess.run(
        command, cwd=REPO_ROOT, text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or f"git diff failed for {base_ref}")
    return result.stdout


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lcov")
    parser.add_argument("--base-ref", required=True)
    parser.add_argument("--threshold", type=float, default=95.0)
    parser.add_argument("--json-output")
    parser.add_argument("--no-gate", action="store_true")
    args = parser.parse_args(argv)

    lcov_path = Path(args.lcov)
    if not lcov_path.is_file():
        print(f"error: LCOV report not found: {lcov_path}", file=sys.stderr)
        return 2
    try:
        changed = exclude_inline_test_modules(parse_diff(git_diff(args.base_ref)))
        coverage = parse_lcov(lcov_path.read_text(encoding="utf-8"))
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    report = evaluate_changed_coverage(changed, coverage)
    if args.json_output:
        output = Path(args.json_output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(
        "Changed production Rust line coverage: "
        f"{report['percent']:.2f}% ({report['covered']}/{report['total']})"
    )
    for entry in report["files"]:
        missed = entry["missed_lines"]
        suffix = f" missed={','.join(map(str, missed))}" if missed else ""
        print(
            f"  {entry['file']}: {entry['covered']}/{entry['total']}" + suffix
        )

    if args.no_gate or report["total"] == 0:
        if report["total"] == 0:
            print("No changed instrumentable production Rust lines.")
        return 0
    if report["percent"] + 1e-9 < args.threshold:
        print(
            f"CHANGED-LINES GATE: FAIL ({report['percent']:.2f}% < {args.threshold:.2f}%)"
        )
        return 1
    print(f"CHANGED-LINES GATE: PASS (threshold {args.threshold:.2f}%)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
