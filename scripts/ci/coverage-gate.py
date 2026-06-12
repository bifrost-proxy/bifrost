#!/usr/bin/env python3
"""Coverage gate & gap analysis for the Bifrost workspace.

Consumes the JSON produced by ``cargo llvm-cov ... --json`` (or by
``llvm-cov export -format=text``) and enforces the per-crate / workspace floors
declared in ``scripts/ci/coverage-thresholds.toml``.

Two modes (combinable):

  * gate (default): exit non-zero if any crate or the workspace aggregate is
    below its configured floor. This is what CI runs.

  * --gaps: print a prioritized list of the files furthest below the target,
    with uncovered-line counts, so contributors know exactly where to add
    tests to reach 90%.

Usage:
    python3 scripts/ci/coverage-gate.py COVERAGE_JSON [options]

Options:
    --thresholds FILE   Threshold manifest (default: scripts/ci/coverage-thresholds.toml)
    --gaps              Print gap analysis (lowest-covered files first)
    --top N             Limit gap analysis to N files (default: 30)
    --no-gate           Skip enforcement (report only); always exit 0
    --markdown          Emit a Markdown summary table (for CI job summaries)
    -h, --help          Show this help

Exit codes:
    0  all floors satisfied (or --no-gate)
    1  at least one floor violated
    2  bad input (missing file / parse error)
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from collections import defaultdict

# tomllib is stdlib on 3.11+; fall back to a tiny parser for the simple manifest.
try:
    import tomllib  # type: ignore
except ModuleNotFoundError:  # pragma: no cover - exercised only on <3.11
    tomllib = None


REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def _eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def load_thresholds(path: str) -> dict:
    if tomllib is not None:
        with open(path, "rb") as fh:
            return tomllib.load(fh)
    # Minimal fallback parser (handles the flat structure of our manifest).
    data: dict = {"settings": {}, "crates": {}}
    section = None
    with open(path, "r", encoding="utf-8") as fh:
        for raw in fh:
            line = raw.split("#", 1)[0].strip()
            if not line:
                continue
            if line.startswith("[crates."):
                section = ("crates", line[len("[crates."):-1])
                data["crates"].setdefault(section[1], {})
            elif line == "[settings]":
                section = ("settings", None)
            elif "=" in line and section is not None:
                key, _, val = line.partition("=")
                key, val = key.strip(), val.strip()
                try:
                    parsed: object = float(val)
                except ValueError:
                    parsed = val.strip('"').lower() in ("true", "1", "yes")
                if section[0] == "settings":
                    data["settings"][key] = parsed
                else:
                    data["crates"][section[1]][key] = parsed
    return data


def crate_for_file(filename: str) -> str | None:
    """Map an absolute source path to its workspace crate name.

    Recognizes both ``crates/<name>/...`` and the integration tests under the
    repo-root ``tests/`` dir (which belong to the ``bifrost-tests`` crate).
    """
    norm = filename.replace("\\", "/")
    marker = "/crates/"
    idx = norm.rfind(marker)
    if idx != -1:
        rest = norm[idx + len(marker):]
        return rest.split("/", 1)[0]
    # Root integration tests compiled under the bifrost-tests crate.
    if "/tests/" in norm and "/crates/" not in norm:
        return "bifrost-tests"
    return None


def parse_llvm_json(path: str):
    """Return (per_file_summary, per_crate_totals, workspace_totals).

    per_file_summary: list of dicts {file, crate, covered, count, percent, missed}
    per_crate_totals: {crate: {covered, count}}
    workspace_totals: {covered, count}
    """
    with open(path, "r", encoding="utf-8") as fh:
        doc = json.load(fh)

    files_out = []
    crate_tot: dict[str, dict[str, int]] = defaultdict(lambda: {"covered": 0, "count": 0})
    ws = {"covered": 0, "count": 0}

    data_block = doc["data"][0]
    for f in data_block.get("files", []):
        name = f["filename"]
        # Skip vendored/registry/std sources defensively.
        if "/registry/" in name or "/rustc/" in name or "/.cargo/" in name:
            continue
        lines = f["summary"]["lines"]
        covered = int(lines["covered"])
        count = int(lines["count"])
        if count == 0:
            continue
        crate = crate_for_file(name) or "(unknown)"
        files_out.append(
            {
                "file": name,
                "crate": crate,
                "covered": covered,
                "count": count,
                "percent": (covered / count * 100.0) if count else 100.0,
                "missed": count - covered,
            }
        )
        crate_tot[crate]["covered"] += covered
        crate_tot[crate]["count"] += count
        ws["covered"] += covered
        ws["count"] += count

    return files_out, crate_tot, ws


def pct(covered: int, count: int) -> float:
    return (covered / count * 100.0) if count else 100.0


def run_gate(crate_tot, ws, thresholds, skip_workspace=False) -> tuple[bool, list[str]]:
    settings = thresholds.get("settings", {})
    default = float(settings.get("default", 90.0))
    ws_min = float(settings.get("workspace_min", default))
    crate_cfg = thresholds.get("crates", {})

    failures: list[str] = []
    ok = True

    # Workspace aggregate (skipped for single-crate runs, where the aggregate is
    # not representative of the whole workspace).
    if not skip_workspace:
        ws_pct = pct(ws["covered"], ws["count"])
        if ws_pct + 1e-9 < ws_min:
            ok = False
            failures.append(
                f"workspace {ws_pct:.2f}% < floor {ws_min:.2f}% "
                f"({ws['covered']}/{ws['count']} lines)"
            )

    # Per-crate floors.
    for crate, tot in sorted(crate_tot.items()):
        floor = float(crate_cfg.get(crate, {}).get("min", default))
        p = pct(tot["covered"], tot["count"])
        if p + 1e-9 < floor:
            ok = False
            failures.append(
                f"crate {crate:22s} {p:6.2f}% < floor {floor:.2f}% "
                f"({tot['covered']}/{tot['count']} lines)"
            )
    return ok, failures


def print_crate_table(crate_tot, thresholds, markdown=False):
    settings = thresholds.get("settings", {})
    default = float(settings.get("default", 90.0))
    crate_cfg = thresholds.get("crates", {})

    rows = []
    for crate, tot in sorted(crate_tot.items()):
        floor = float(crate_cfg.get(crate, {}).get("min", default))
        p = pct(tot["covered"], tot["count"])
        status = "PASS" if p + 1e-9 >= floor else "FAIL"
        rows.append((crate, p, floor, tot["covered"], tot["count"], status))

    if markdown:
        print("| Crate | Coverage | Floor | Lines | Status |")
        print("|---|---:|---:|---:|---|")
        for crate, p, floor, cov, cnt, status in rows:
            emoji = "✅" if status == "PASS" else "❌"
            print(f"| `{crate}` | {p:.2f}% | {floor:.0f}% | {cov}/{cnt} | {emoji} |")
    else:
        print(f"{'Crate':24s}{'Cover':>9s}{'Floor':>8s}{'Lines':>14s}  Status")
        print("-" * 70)
        for crate, p, floor, cov, cnt, status in rows:
            print(f"{crate:24s}{p:8.2f}%{floor:7.0f}%{cov:>7d}/{cnt:<6d}  {status}")


def print_gaps(files_out, thresholds, top):
    default = float(thresholds.get("settings", {}).get("default", 90.0))
    # Files below target, sorted by uncovered-line count (biggest wins first),
    # then by how far below target they are.
    below = [f for f in files_out if f["percent"] + 1e-9 < default]
    below.sort(key=lambda f: (-f["missed"], f["percent"]))
    print()
    print(f"=== Coverage gaps (target {default:.0f}%) — {len(below)} files below target ===")
    print(f"{'Uncov':>6s} {'Cover':>7s}  File")
    print("-" * 90)
    for f in below[:top]:
        rel = os.path.relpath(f["file"], REPO_ROOT)
        print(f"{f['missed']:6d} {f['percent']:6.1f}%  {rel}")
    if len(below) > top:
        print(f"... and {len(below) - top} more (use --top N to see more)")
    # Per-crate uncovered totals: where to invest.
    by_crate: dict[str, int] = defaultdict(int)
    for f in below:
        by_crate[f["crate"]] += f["missed"]
    print()
    print("=== Uncovered lines by crate (investment priority) ===")
    for crate, missed in sorted(by_crate.items(), key=lambda kv: -kv[1]):
        print(f"  {missed:7d}  {crate}")


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(add_help=True, description=__doc__)
    ap.add_argument("coverage_json")
    ap.add_argument(
        "--thresholds",
        default=os.path.join(REPO_ROOT, "scripts", "ci", "coverage-thresholds.toml"),
    )
    ap.add_argument("--gaps", action="store_true")
    ap.add_argument("--top", type=int, default=30)
    ap.add_argument("--no-gate", action="store_true")
    ap.add_argument(
        "--single-crate",
        action="store_true",
        help="Skip the workspace-aggregate floor (for -p single-crate runs)",
    )
    ap.add_argument("--markdown", action="store_true")
    args = ap.parse_args(argv)

    if not os.path.exists(args.coverage_json):
        _eprint(f"error: coverage JSON not found: {args.coverage_json}")
        return 2
    try:
        thresholds = load_thresholds(args.thresholds)
    except Exception as exc:  # noqa: BLE001
        _eprint(f"error: failed to read thresholds {args.thresholds}: {exc}")
        return 2
    try:
        files_out, crate_tot, ws = parse_llvm_json(args.coverage_json)
    except Exception as exc:  # noqa: BLE001
        _eprint(f"error: failed to parse coverage JSON: {exc}")
        return 2

    ws_pct = pct(ws["covered"], ws["count"])
    print(f"Workspace line coverage: {ws_pct:.2f}% ({ws['covered']}/{ws['count']})")
    print()
    print_crate_table(crate_tot, thresholds, markdown=args.markdown)

    if args.gaps:
        print_gaps(files_out, thresholds, args.top)

    if args.no_gate:
        return 0

    ok, failures = run_gate(crate_tot, ws, thresholds, skip_workspace=args.single_crate)
    print()
    if ok:
        print("COVERAGE GATE: PASS")
        return 0
    print("COVERAGE GATE: FAIL")
    for f in failures:
        print(f"  - {f}")
    return 1


if __name__ == "__main__":
    sys.exit(main())
