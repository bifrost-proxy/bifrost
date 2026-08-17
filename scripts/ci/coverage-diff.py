#!/usr/bin/env python3
"""Enforce line coverage for changed production Rust lines.

The workspace ratchet prevents global regression, but a large crate can still
absorb untested new code.  This gate intersects added lines from ``git diff``
with instrumentable lines from LCOV and applies a stricter PR threshold.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import subprocess
import sys
from collections import defaultdict
from functools import lru_cache
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
HUNK_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@")
PRODUCTION_RUST_RE = re.compile(r"^crates/[^/]+/src/.+\.rs$")
EXTERNAL_TEST_MODULE_RE = re.compile(
    r"#\[cfg\(test\)\](?P<attrs>(?:\s*#\[[^\]]+\])*)\s*"
    r"(?:pub(?:\([^)]*\))?\s+)?mod\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*;",
    re.MULTILINE,
)
PATH_ATTR_RE = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
MOVED_BLOCK_MIN_LINES = 8
MOVED_BLOCK_MIN_SUBSTANTIVE_LINES = 4


@lru_cache(maxsize=None)
def external_test_module_roots(repo_root: Path) -> tuple[Path, ...]:
    """Resolve only modules actually declared behind exact ``#[cfg(test)]``."""
    roots: set[Path] = set()
    crates_root = repo_root / "crates"
    if not crates_root.is_dir():
        return ()
    for source_path in crates_root.glob("*/src/**/*.rs"):
        source = source_path.read_text(encoding="utf-8")
        module_dir = (
            source_path.parent
            if source_path.stem in {"lib", "main", "mod"}
            else source_path.parent / source_path.stem
        )
        for declaration in EXTERNAL_TEST_MODULE_RE.finditer(source):
            explicit_path = PATH_ATTR_RE.search(declaration.group("attrs"))
            if explicit_path is not None:
                module_file = (source_path.parent / explicit_path.group(1)).resolve()
                roots.add(module_file)
                if module_file.suffix == ".rs":
                    roots.add(module_file.with_suffix(""))
                continue
            module_name = declaration.group("name")
            roots.add((module_dir / f"{module_name}.rs").resolve())
            roots.add((module_dir / module_name).resolve())
    return tuple(sorted(roots))


def is_production_rust_path(path: str, repo_root: Path = REPO_ROOT) -> bool:
    """Exclude verified external ``#[cfg(test)]`` modules, not path names."""
    if not PRODUCTION_RUST_RE.match(path):
        return False
    absolute = (repo_root / path).resolve()
    return not any(
        absolute == root or root in absolute.parents
        for root in external_test_module_roots(repo_root.resolve())
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


def rust_non_executable_lines(source: str) -> set[int]:
    """Return comment, attribute, and punctuation-only Rust source lines.

    LLVM can attach a zero-hit region to a comment or closing delimiter when a
    changed multi-line expression spans it. Those lines do not represent an
    executable coverage obligation; counting them makes formatting affect the
    changed-lines gate. Statements that contain identifiers or literals remain
    eligible and therefore still require real execution coverage.
    """
    excluded: set[int] = set()
    in_block_comment = False
    declaration_depth = 0
    in_signature = False
    in_static = False
    in_matches_macro = 0
    in_tracing_macro = 0
    for line_no, line in enumerate(source.splitlines(), start=1):
        stripped = line.strip()
        if in_tracing_macro > 0:
            in_tracing_macro += line.count("(") - line.count(")")
            if re.fullmatch(r'"(?:[^"\\]|\\.)*",?', stripped):
                # The message argument is passive metadata for the tracing
                # invocation. The invocation line remains the branch anchor.
                excluded.add(line_no)
                continue
        if in_matches_macro > 0:
            # `matches!` alternatives and their comma-separated input are one
            # expression. LLVM assigns the executable region to the macro
            # invocation/condition, while variant-only continuation lines get
            # zero-hit formatting regions.
            excluded.add(line_no)
            in_matches_macro += line.count("(") - line.count(")")
            continue
        if declaration_depth > 0:
            excluded.add(line_no)
            declaration_depth += line.count("{") - line.count("}")
            continue
        if in_signature:
            excluded.add(line_no)
            if "{" in line or stripped.endswith(";"):
                in_signature = False
            continue
        if in_static:
            excluded.add(line_no)
            if stripped.endswith(";"):
                in_static = False
            continue
        if in_block_comment:
            excluded.add(line_no)
            if "*/" in stripped:
                in_block_comment = False
            continue
        if not stripped or stripped.startswith("//"):
            excluded.add(line_no)
            continue
        if stripped.startswith("/*"):
            excluded.add(line_no)
            in_block_comment = "*/" not in stripped
            continue
        if stripped.startswith("#[") and stripped.endswith("]"):
            excluded.add(line_no)
            continue
        if re.match(r"^(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum)\s+", stripped):
            excluded.add(line_no)
            declaration_depth = line.count("{") - line.count("}")
            continue
        if re.match(r"^(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+", stripped):
            excluded.add(line_no)
            if "{" not in line and not stripped.endswith(";"):
                in_signature = True
            continue
        if re.match(r"^(?:unsafe\s+)?impl(?:<[^>]*>)?\s+.*\{\s*$", stripped):
            # An `impl` header declares the following body. Its methods remain
            # independently measurable, but the header itself cannot execute.
            excluded.add(line_no)
            continue
        if re.match(r"^(?:pub(?:\([^)]*\))?\s+)?(?:static|const)\s+", stripped):
            excluded.add(line_no)
            in_static = not stripped.endswith(";")
            continue
        if "matches!(" in stripped and line.count("(") > line.count(")"):
            in_matches_macro = line.count("(") - line.count(")")
            continue
        if re.search(r"\btracing::[A-Za-z_][A-Za-z0-9_]*!\(", stripped):
            in_tracing_macro = line.count("(") - line.count(")")
        # A chained method continuation is formatting for the expression that
        # starts on the preceding line. LLVM normally attributes execution to
        # that anchor and emits zero-hit regions for one or more `.method(...)`
        # continuation lines.
        if stripped.startswith("."):
            excluded.add(line_no)
            continue
        # Match-arm headers select control flow but are not executable source
        # statements themselves.
        if stripped.endswith("=>"):
            excluded.add(line_no)
            continue
        # Shorthand arguments and tracing fields inside multi-line calls/macros
        # do not form independent statements. Keep calls, assignments, awaits,
        # closures, and conditionals eligible; exclude only passive values.
        if re.fullmatch(r"&?[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*,", stripped):
            excluded.add(line_no)
            continue
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*\s*=\s*.+,", stripped):
            excluded.add(line_no)
            continue
        if re.fullmatch(r'"(?:[^"\\]|\\.)*"\s*:\s*.+,', stripped):
            # `serde_json!` object fields are formatter-split continuations of
            # the macro invocation, just like ordinary struct fields below.
            excluded.add(line_no)
            continue
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*:\s*.+,", stripped):
            # A field line belongs to the surrounding struct or macro
            # expression. Keep that expression's opening line eligible so an
            # unentered branch still fails coverage; only the formatter-split
            # field continuation is ignored.
            excluded.add(line_no)
            continue
        if re.fullmatch(r"\([^=]*,\s*_\)", stripped):
            # Multi-line tuple match-arm patterns only declare branch
            # selectors; the guarded arm body remains a coverage obligation.
            excluded.add(line_no)
            continue
        if re.fullmatch(r"[{}()\[\],;]+", stripped):
            excluded.add(line_no)
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
        excluded.update(
            rust_non_executable_lines(source_path.read_text(encoding="utf-8"))
        )
        filtered[path] = set(lines).difference(excluded)
    return filtered


def unchanged_moved_block_lines(
    current_source: str,
    base_sources: list[tuple[str, str, str]],
    current_path: str,
) -> set[int]:
    """Return current line numbers copied unchanged from substantial base blocks.

    File splits and module extraction should not turn already-existing production
    code into "new" code for the changed-lines ratchet.  Only contiguous exact
    matches of at least eight lines, containing at least four non-comment lines
    with identifiers, qualify.  Small boilerplate matches remain gated.
    """
    current_lines = current_source.splitlines()
    moved: set[int] = set()
    for base_path, base_source, original_current_source in base_sources:
        base_lines = base_source.splitlines()
        same_file = base_path == current_path
        retained_base_lines: set[int] = set()
        if not same_file:
            retained_matcher = difflib.SequenceMatcher(
                None,
                base_lines,
                original_current_source.splitlines(),
                autojunk=False,
            )
            for retained in retained_matcher.get_matching_blocks():
                # Every exact line still present at the source prevents that
                # line from qualifying as deleted-and-moved.  The substantial
                # block threshold belongs only to the destination candidate;
                # applying it here lets one edited middle line split retained
                # source code into smaller runs and hide a copied suffix.
                retained_base_lines.update(range(retained.a, retained.a + retained.size))
        matcher = difflib.SequenceMatcher(
            None, base_lines, current_lines, autojunk=False
        )
        for block in matcher.get_matching_blocks():
            if block.size < MOVED_BLOCK_MIN_LINES:
                continue
            matched = current_lines[block.b : block.b + block.size]
            if same_file and sequence_occurrence_count(
                current_lines, matched
            ) > sequence_occurrence_count(base_lines, matched):
                continue
            substantive = sum(
                1
                for line in matched
                if not line.lstrip().startswith("//")
                and re.search(r"[A-Za-z0-9_]", line)
            )
            if substantive < MOVED_BLOCK_MIN_SUBSTANTIVE_LINES:
                continue
            run_start: int | None = None
            for offset in range(block.size + 1):
                unretained = (
                    offset < block.size
                    and block.a + offset not in retained_base_lines
                )
                if unretained and run_start is None:
                    run_start = offset
                if unretained or run_start is None:
                    continue
                run = current_lines[block.b + run_start : block.b + offset]
                run_substantive = sum(
                    1
                    for line in run
                    if not line.lstrip().startswith("//")
                    and re.search(r"[A-Za-z0-9_]", line)
                )
                if (
                    len(run) >= MOVED_BLOCK_MIN_LINES
                    and run_substantive >= MOVED_BLOCK_MIN_SUBSTANTIVE_LINES
                ):
                    moved.update(range(block.b + run_start + 1, block.b + offset + 1))
                run_start = None
    return moved


def sequence_occurrence_count(lines: list[str], block: list[str]) -> int:
    """Count exact, potentially overlapping occurrences of a line block."""
    if not block or len(block) > len(lines):
        return 0
    return sum(
        lines[index : index + len(block)] == block
        for index in range(len(lines) - len(block) + 1)
    )


def exclude_unchanged_moved_blocks(
    changed: dict[str, set[int]],
    base_sources: list[tuple[str, str, str]],
    repo_root: Path = REPO_ROOT,
) -> tuple[dict[str, set[int]], int]:
    filtered: dict[str, set[int]] = {}
    excluded_count = 0
    for path, lines in changed.items():
        source_path = repo_root / path
        if not PRODUCTION_RUST_RE.match(path) or not source_path.is_file():
            filtered[path] = set(lines)
            continue
        moved = unchanged_moved_block_lines(
            source_path.read_text(encoding="utf-8"), base_sources, path
        )
        filtered[path] = set(lines).difference(moved)
        excluded_count += len(set(lines).intersection(moved))
    return filtered, excluded_count


def evaluate_changed_coverage(
    changed: dict[str, set[int]], coverage: dict[str, dict[int, int]]
) -> dict[str, object]:
    files: list[dict[str, object]] = []
    total = 0
    covered = 0
    for path in sorted(changed):
        if not is_production_rust_path(path):
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


def unmeasured_changed_files(
    changed: dict[str, set[int]], coverage: dict[str, dict[int, int]]
) -> list[str]:
    return sorted(
        path
        for path, lines in changed.items()
        if lines
        if is_production_rust_path(path) and path not in coverage
    )


def untracked_production_diff(repo_root: Path | None = None) -> str:
    repo_root = repo_root or REPO_ROOT
    names = subprocess.run(
        [
            "git",
            "ls-files",
            "--others",
            "--exclude-standard",
            "--",
            "crates/*/src/*.rs",
            "crates/*/src/**/*.rs",
        ],
        cwd=repo_root,
        text=True,
        capture_output=True,
        check=False,
    )
    if names.returncode != 0:
        raise RuntimeError(names.stderr.strip() or "git ls-files failed")

    diffs: list[str] = []
    for path in names.stdout.splitlines():
        normalized = normalize_path(path, repo_root)
        if not is_production_rust_path(normalized):
            continue
        result = subprocess.run(
            ["git", "diff", "--no-index", "--unified=0", "/dev/null", normalized],
            cwd=repo_root,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode not in (0, 1):
            raise RuntimeError(result.stderr.strip() or f"git diff failed for {path}")
        diffs.append(result.stdout)
    return "".join(diffs)


def git_diff(base_ref: str, *, worktree: bool = False) -> str:
    revision = f"{base_ref}...HEAD"
    if worktree:
        merge_base = subprocess.run(
            ["git", "merge-base", base_ref, "HEAD"],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        if merge_base.returncode != 0:
            raise RuntimeError(
                merge_base.stderr.strip() or f"git merge-base failed for {base_ref}"
            )
        revision = merge_base.stdout.strip()
    command = [
        "git",
        "diff",
        "--unified=0",
        "--diff-filter=AM",
        revision,
        "--",
        "crates/*/src/*.rs",
        "crates/*/src/**/*.rs",
    ]
    result = subprocess.run(
        command, cwd=REPO_ROOT, text=True, capture_output=True, check=False
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or f"git diff failed for {base_ref}")
    diff = result.stdout
    if worktree:
        diff += untracked_production_diff(REPO_ROOT)
    return diff


def changed_base_sources(
    base_ref: str, *, worktree: bool = False
) -> list[tuple[str, str, str]]:
    merge_base = subprocess.run(
        ["git", "merge-base", base_ref, "HEAD"],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    if merge_base.returncode != 0:
        raise RuntimeError(
            merge_base.stderr.strip() or f"git merge-base failed for {base_ref}"
        )
    base_commit = merge_base.stdout.strip()
    revisions = [base_commit] if worktree else [base_commit, "HEAD"]
    names = subprocess.run(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=DM",
            *revisions,
            "--",
            "crates/*/src/*.rs",
            "crates/*/src/**/*.rs",
        ],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    if names.returncode != 0:
        raise RuntimeError(names.stderr.strip() or "git diff --name-only failed")

    sources: list[tuple[str, str, str]] = []
    for path in names.stdout.splitlines():
        normalized = normalize_path(path)
        if not is_production_rust_path(normalized):
            continue
        content = subprocess.run(
            ["git", "show", f"{base_commit}:{normalized}"],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        if content.returncode == 0:
            current_path = REPO_ROOT / normalized
            current = (
                current_path.read_text(encoding="utf-8")
                if current_path.is_file()
                else ""
            )
            sources.append((normalized, content.stdout, current))
    return sources


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lcov")
    parser.add_argument("--base-ref", required=True)
    parser.add_argument("--threshold", type=float, default=90.0)
    parser.add_argument("--json-output")
    parser.add_argument("--no-gate", action="store_true")
    parser.add_argument(
        "--worktree",
        action="store_true",
        help="include staged, unstaged, and untracked files instead of base...HEAD only",
    )
    args = parser.parse_args(argv)

    lcov_path = Path(args.lcov)
    if not lcov_path.is_file():
        print(f"error: LCOV report not found: {lcov_path}", file=sys.stderr)
        return 2
    try:
        changed = parse_diff(git_diff(args.base_ref, worktree=args.worktree))
        changed, moved_lines_excluded = exclude_unchanged_moved_blocks(
            changed, changed_base_sources(args.base_ref, worktree=args.worktree)
        )
        changed = exclude_inline_test_modules(changed)
        coverage = parse_lcov(lcov_path.read_text(encoding="utf-8"))
    except (OSError, RuntimeError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    report = evaluate_changed_coverage(changed, coverage)
    production_files = sorted(path for path in changed if is_production_rust_path(path))
    report["changed_production_files"] = production_files
    report["unmeasured_files"] = unmeasured_changed_files(changed, coverage)
    report["unchanged_moved_lines_excluded"] = moved_lines_excluded
    if args.json_output:
        output = Path(args.json_output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print(
        "Changed production Rust line coverage: "
        f"{report['percent']:.2f}% ({report['covered']}/{report['total']})"
    )
    if moved_lines_excluded:
        print(f"Unchanged removed-and-moved Rust lines excluded: {moved_lines_excluded}")
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
