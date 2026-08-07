#!/usr/bin/env python3
"""Run a low-cost local coverage preflight for changed production Rust code.

The full workspace + proxy E2E coverage job remains authoritative in CI.  This
tool gives developers fast feedback by instrumenting only crates that contain
changed production Rust files, then applying the same changed-lines threshold
to the current working tree (including staged, unstaged, and untracked files).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any, Sequence


REPO_ROOT = Path(__file__).resolve().parents[2]
PRODUCTION_RUST_RE = re.compile(r"^crates/[^/]+/src/.+\.rs$")
CHANGED_LINES_MIN_RE = re.compile(
    r"^\s*changed_lines_min\s*=\s*([0-9]+(?:\.[0-9]+)?)\s*(?:#.*)?$"
)


class PreflightError(RuntimeError):
    """A user-actionable local preflight configuration error."""


def run(
    command: Sequence[str],
    *,
    cwd: Path,
    env: dict[str, str] | None = None,
    capture_output: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        list(command),
        cwd=cwd,
        env=env,
        text=True,
        capture_output=capture_output,
        check=False,
    )


def resolve_base_ref(repo_root: Path, explicit: str | None) -> str:
    candidates = [
        explicit,
        os.environ.get("COVERAGE_BASE_REF"),
        "origin/main",
        "main",
    ]
    for candidate in candidates:
        if not candidate:
            continue
        result = run(
            ["git", "rev-parse", "--verify", "--quiet", candidate], cwd=repo_root
        )
        if result.returncode == 0:
            return candidate
        if explicit or os.environ.get("COVERAGE_BASE_REF") == candidate:
            raise PreflightError(f"coverage base ref does not exist: {candidate}")
    raise PreflightError(
        "cannot resolve coverage base ref; pass --base-ref <ref> or fetch origin/main"
    )


def merge_base(repo_root: Path, base_ref: str) -> str:
    result = run(["git", "merge-base", base_ref, "HEAD"], cwd=repo_root)
    if result.returncode != 0:
        raise PreflightError(result.stderr.strip() or f"git merge-base failed: {base_ref}")
    return result.stdout.strip()


def changed_production_paths(
    repo_root: Path, base_ref: str, *, worktree: bool
) -> list[str]:
    diff_base = merge_base(repo_root, base_ref)
    revision = diff_base if worktree else f"{base_ref}...HEAD"
    result = run(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=AM",
            revision,
            "--",
            "crates/*/src/*.rs",
            "crates/*/src/**/*.rs",
        ],
        cwd=repo_root,
    )
    if result.returncode != 0:
        raise PreflightError(result.stderr.strip() or "git diff failed")

    paths = set(result.stdout.splitlines())
    if worktree:
        untracked = run(
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
        )
        if untracked.returncode != 0:
            raise PreflightError(untracked.stderr.strip() or "git ls-files failed")
        paths.update(untracked.stdout.splitlines())
    return sorted(path for path in paths if PRODUCTION_RUST_RE.match(path))


def load_metadata(repo_root: Path) -> dict[str, Any]:
    result = run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=repo_root
    )
    if result.returncode != 0:
        raise PreflightError(result.stderr.strip() or "cargo metadata failed")
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as exc:
        raise PreflightError(f"cargo metadata returned invalid JSON: {exc}") from exc


def packages_for_paths(
    paths: Sequence[str], metadata: dict[str, Any], repo_root: Path
) -> list[str]:
    package_roots: list[tuple[Path, str]] = []
    workspace_members = set(metadata.get("workspace_members", []))
    for package in metadata.get("packages", []):
        if package.get("id") not in workspace_members:
            continue
        manifest = Path(package["manifest_path"]).resolve()
        package_roots.append((manifest.parent, package["name"]))

    selected: set[str] = set()
    unresolved: list[str] = []
    for path in paths:
        absolute = (repo_root / path).resolve()
        matches = [
            (root, name)
            for root, name in package_roots
            if absolute == root or root in absolute.parents
        ]
        if not matches:
            unresolved.append(path)
            continue
        selected.add(max(matches, key=lambda item: len(item[0].parts))[1])
    if unresolved:
        raise PreflightError(
            "cannot map changed Rust files to workspace packages: "
            + ", ".join(unresolved)
        )
    return sorted(selected)


def changed_lines_threshold(path: Path) -> float:
    for line in path.read_text(encoding="utf-8").splitlines():
        match = CHANGED_LINES_MIN_RE.match(line)
        if match:
            return float(match.group(1))
    raise PreflightError(f"changed_lines_min is missing from {path}")


def coverage_command(
    packages: Sequence[str],
    *,
    output_path: Path,
    jobs: int,
    test_filter: str | None,
) -> list[str]:
    command = ["cargo", "llvm-cov", "--no-clean"]
    for package in packages:
        command.extend(["--package", package])
    command.extend(
        [
            "--all-features",
            "--lcov",
            "--output-path",
            str(output_path),
            "--jobs",
            str(jobs),
        ]
    )
    if test_filter:
        command.extend(["--", test_filter])
    return command


def clear_profiles(target_dir: Path) -> int:
    removed = 0
    if not target_dir.exists():
        return removed
    for profile in target_dir.rglob("*.profraw"):
        profile.unlink()
        removed += 1
    return removed


def print_command(command: Sequence[str]) -> None:
    import shlex

    print("+ " + " ".join(shlex.quote(part) for part in command), flush=True)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-ref", help="comparison ref (default: origin/main)")
    parser.add_argument(
        "--committed",
        action="store_true",
        help="check base...HEAD only instead of the current working tree",
    )
    parser.add_argument(
        "--package",
        action="append",
        default=[],
        help="override auto-detected packages (repeatable)",
    )
    parser.add_argument(
        "--include-package",
        action="append",
        default=[],
        help="add a package whose tests cover the changed crates (repeatable)",
    )
    parser.add_argument(
        "--test-filter", help="pass one focused test-name filter to cargo test"
    )
    parser.add_argument("--threshold", type=float, help="changed-lines percentage")
    parser.add_argument(
        "--allow-unmeasured",
        action="store_true",
        help="allow changed files absent from local LCOV (must be justified)",
    )
    parser.add_argument("--jobs", type=int, default=int(os.environ.get("COVERAGE_JOBS", "4")))
    parser.add_argument(
        "--output-dir", default="target/coverage-changed", help="report/cache directory"
    )
    parser.add_argument(
        "--plan", action="store_true", help="print selection and command without running tests"
    )
    args = parser.parse_args(argv)

    if args.jobs < 1:
        parser.error("--jobs must be at least 1")
    if args.threshold is not None and not 0 <= args.threshold <= 100:
        parser.error("--threshold must be between 0 and 100")

    repo_root = REPO_ROOT
    try:
        base_ref = resolve_base_ref(repo_root, args.base_ref)
        worktree = not args.committed
        changed_paths = changed_production_paths(repo_root, base_ref, worktree=worktree)
        if not changed_paths:
            scope = "working tree" if worktree else "base...HEAD"
            print(f"No changed production Rust files in {scope}; coverage preflight skipped.")
            return 0

        metadata = load_metadata(repo_root)
        detected = packages_for_paths(changed_paths, metadata, repo_root)
        packages = sorted(set(args.package or detected).union(args.include_package))
        missing = sorted(set(detected).difference(packages))
        if missing:
            raise PreflightError(
                "--package omitted changed package(s): "
                + ", ".join(missing)
                + "; include every changed package or use auto-detection"
            )

        output_dir = (repo_root / args.output_dir).resolve()
        target_dir = output_dir / "cargo-target"
        lcov_path = output_dir / "lcov.info"
        report_path = output_dir / "changed-lines.json"
        threshold = args.threshold or changed_lines_threshold(
            repo_root / "scripts/ci/coverage-thresholds.toml"
        )
        command = coverage_command(
            packages,
            output_path=lcov_path,
            jobs=args.jobs,
            test_filter=args.test_filter,
        )

        print(f"Base ref : {base_ref}")
        print(f"Scope    : {'working tree' if worktree else 'committed HEAD'}")
        print(f"Packages : {', '.join(packages)}")
        print(f"Threshold: {threshold:.2f}%")
        if args.test_filter:
            print(
                "Mode     : focused iteration only; rerun without --test-filter before push"
            )
        print("Changed production Rust files:")
        for path in changed_paths:
            print(f"  - {path}")
        print_command(command)
        if args.plan:
            return 0

        if shutil.which("cargo-llvm-cov") is None:
            raise PreflightError(
                "cargo-llvm-cov is not installed; run `cargo install cargo-llvm-cov`"
            )

        output_dir.mkdir(parents=True, exist_ok=True)
        target_dir.mkdir(parents=True, exist_ok=True)
        removed = clear_profiles(target_dir)
        if removed:
            print(f"Reset {removed} stale coverage profile(s); kept compiled artifacts.")

        env = os.environ.copy()
        env.update(
            {
                "BIFROST_DISABLE_TRAY": "1",
                "BIFROST_SYNC_DISABLE_AUTO_LOGIN_PROMPT": "1",
                "CARGO_BUILD_JOBS": str(args.jobs),
                "CARGO_INCREMENTAL": "0",
                "CARGO_LLVM_COV_BUILD_DIR": str(target_dir),
                "CARGO_LLVM_COV_TARGET_DIR": str(target_dir),
                "CARGO_TARGET_DIR": str(target_dir),
                "RAYON_NUM_THREADS": str(args.jobs),
                "SKIP_FRONTEND_BUILD": "1",
            }
        )
        collected = run(command, cwd=repo_root, env=env, capture_output=False)
        if collected.returncode != 0:
            return collected.returncode

        diff_command = [
            sys.executable,
            str(repo_root / "scripts/ci/coverage-diff.py"),
            str(lcov_path),
            "--base-ref",
            base_ref,
            "--threshold",
            str(threshold),
            "--json-output",
            str(report_path),
        ]
        if worktree:
            diff_command.append("--worktree")
        print_command(diff_command)
        evaluated = run(diff_command, cwd=repo_root, capture_output=False)
        if not report_path.is_file():
            return evaluated.returncode or 2

        report = json.loads(report_path.read_text(encoding="utf-8"))
        unmeasured = report.get("unmeasured_files", [])
        if unmeasured:
            print("Changed files absent from local LCOV:", file=sys.stderr)
            for path in unmeasured:
                print(f"  - {path}", file=sys.stderr)
            if not args.allow_unmeasured:
                print(
                    "LOCAL PREFLIGHT: INCOMPLETE. Add direct tests, include the test-host "
                    "package with --include-package, or justify --allow-unmeasured. "
                    "CI remains authoritative for platform/E2E-only paths.",
                    file=sys.stderr,
                )
                return 2
        return evaluated.returncode
    except (OSError, ValueError, json.JSONDecodeError, PreflightError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
