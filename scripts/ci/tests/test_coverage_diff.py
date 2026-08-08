from __future__ import annotations

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "coverage-diff.py"
SPEC = importlib.util.spec_from_file_location("coverage_diff", SCRIPT)
assert SPEC and SPEC.loader
coverage_diff = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(coverage_diff)


class CoverageDiffTests(unittest.TestCase):
    def test_production_path_filter_verifies_split_test_modules(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/a/src"
            (source / "feature/tests").mkdir(parents=True)
            (source / "lib.rs").write_text(
                "#[cfg(test)]\nmod tests;\nmod runtime_tests;\n",
                encoding="utf-8",
            )
            (source / "tests.rs").write_text("fn helper() {}\n", encoding="utf-8")
            (source / "runtime_tests.rs").write_text(
                "pub fn live() {}\n", encoding="utf-8"
            )
            (source / "feature.rs").write_text(
                '#[cfg(test)]\n#[path = "feature/tests.rs"]\nmod tests;\n',
                encoding="utf-8",
            )
            (source / "feature/tests/nested.rs").write_text(
                "fn helper() {}\n", encoding="utf-8"
            )

            self.assertTrue(
                coverage_diff.is_production_rust_path(
                    "crates/a/src/runtime_tests.rs", root
                )
            )
            self.assertFalse(
                coverage_diff.is_production_rust_path("crates/a/src/tests.rs", root)
            )
            self.assertFalse(
                coverage_diff.is_production_rust_path(
                    "crates/a/src/feature/tests/nested.rs", root
                )
            )

    def git(self, root: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", *args],
            cwd=root,
            text=True,
            capture_output=True,
            check=True,
        )

    def test_parse_diff_tracks_only_added_ranges(self) -> None:
        diff = """diff --git a/crates/a/src/lib.rs b/crates/a/src/lib.rs
+++ b/crates/a/src/lib.rs
@@ -10,0 +11,3 @@
+one
+two
+three
@@ -30,2 +34 @@
+four
"""
        self.assertEqual(
            coverage_diff.parse_diff(diff),
            {"crates/a/src/lib.rs": {11, 12, 13, 34}},
        )

    def test_parse_lcov_normalizes_absolute_workspace_paths(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            report = coverage_diff.parse_lcov(
                f"SF:{root}/crates/a/src/lib.rs\nDA:4,1\nDA:5,0\nend_of_record\n",
                root,
            )
        self.assertEqual(report, {"crates/a/src/lib.rs": {4: 1, 5: 0}})

    def test_evaluate_counts_only_instrumentable_production_lines(self) -> None:
        changed = {
            "crates/a/src/lib.rs": {4, 5, 6},
            "crates/a/tests/integration.rs": {1},
            "docs/guide.md": {1},
        }
        coverage = {"crates/a/src/lib.rs": {4: 1, 5: 0}}
        result = coverage_diff.evaluate_changed_coverage(changed, coverage)
        self.assertEqual(result["covered"], 1)
        self.assertEqual(result["total"], 2)
        self.assertEqual(result["percent"], 50.0)
        self.assertEqual(result["files"][0]["missed_lines"], [5])

    def test_no_coverable_changes_reports_full_coverage(self) -> None:
        result = coverage_diff.evaluate_changed_coverage(
            {"design/coverage-90.md": {1, 2}}, {}
        )
        self.assertEqual(result["total"], 0)
        self.assertEqual(result["percent"], 100.0)

    def test_unmeasured_changed_files_prevent_silent_zero_over_zero(self) -> None:
        self.assertEqual(
            coverage_diff.unmeasured_changed_files(
                {
                    "crates/a/src/platform.rs": {4, 5},
                    "crates/a/tests/integration.rs": {1},
                },
                {},
            ),
            ["crates/a/src/platform.rs"],
        )

    def test_worktree_diff_includes_tracked_and_untracked_rust_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tracked = root / "crates/a/src/lib.rs"
            untracked = root / "crates/a/src/new.rs"
            tracked.parent.mkdir(parents=True)
            tracked.write_text("pub fn existing() {}\n", encoding="utf-8")
            self.git(root, "init", "-b", "main")
            self.git(root, "config", "user.email", "coverage@example.com")
            self.git(root, "config", "user.name", "Coverage Test")
            self.git(root, "add", ".")
            self.git(root, "commit", "-m", "base")
            tracked.write_text(
                "pub fn existing() {\n    println!(\"changed\");\n}\n",
                encoding="utf-8",
            )
            untracked.write_text("pub fn added() {}\n", encoding="utf-8")

            original_root = coverage_diff.REPO_ROOT
            coverage_diff.REPO_ROOT = root
            try:
                diff = coverage_diff.git_diff("main", worktree=True)
            finally:
                coverage_diff.REPO_ROOT = original_root

        changed = coverage_diff.parse_diff(diff)
        self.assertIn("crates/a/src/lib.rs", changed)
        self.assertIn("crates/a/src/new.rs", changed)

    def test_committed_diff_does_not_include_uncommitted_worktree_change(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "crates/a/src/lib.rs"
            source.parent.mkdir(parents=True)
            source.write_text("pub fn existing() {}\n", encoding="utf-8")
            self.git(root, "init", "-b", "main")
            self.git(root, "config", "user.email", "coverage@example.com")
            self.git(root, "config", "user.name", "Coverage Test")
            self.git(root, "add", ".")
            self.git(root, "commit", "-m", "base")
            source.write_text("pub fn changed() {}\n", encoding="utf-8")

            original_root = coverage_diff.REPO_ROOT
            coverage_diff.REPO_ROOT = root
            try:
                diff = coverage_diff.git_diff("main")
            finally:
                coverage_diff.REPO_ROOT = original_root

        self.assertEqual(diff, "")

    def test_inline_cfg_test_module_lines_are_excluded(self) -> None:
        source = """pub fn production() {\n    println!(\"live\");\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn production_works() {\n        super::production();\n    }\n}\n"""
        self.assertEqual(
            coverage_diff.rust_test_module_lines(source), set(range(5, 12))
        )

    def test_exclude_inline_modules_keeps_production_changes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "crates/a/src/lib.rs"
            path.parent.mkdir(parents=True)
            path.write_text(
                "pub fn production() {}\n#[cfg(test)]\nmod tests {\n fn test() {}\n}\n",
                encoding="utf-8",
            )
            filtered = coverage_diff.exclude_inline_test_modules(
                {"crates/a/src/lib.rs": {1, 2, 3, 4, 5}}, root
            )
        self.assertEqual(filtered, {"crates/a/src/lib.rs": {1}})

    def test_substantial_unchanged_moved_block_is_excluded(self) -> None:
        base = """fn download() {
    let client = client();
    let response = client.send();
    if response.is_ok() {
        save(response);
    } else {
        retry(response);
    }
}
"""
        current = base + "new_behavior();\n"
        moved = coverage_diff.unchanged_moved_block_lines(
            current, [("crates/a/src/old.rs", base, "")], "crates/a/src/new.rs"
        )
        self.assertEqual(moved, set(range(1, 10)))
        self.assertNotIn(10, moved)

    def test_small_boilerplate_match_remains_changed(self) -> None:
        source = """if ready {
    return Ok(());
}
"""
        self.assertEqual(
            coverage_diff.unchanged_moved_block_lines(
                source, [("old.rs", source, "")], "new.rs"
            ),
            set(),
        )

    def test_exclude_moved_blocks_reports_only_changed_intersection(self) -> None:
        base = """fn restart() {
    let runtime = read_runtime();
    validate(runtime);
    stop(runtime);
    wait_for_exit(runtime);
    install(runtime);
    start(runtime);
    verify(runtime);
}
"""
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "crates/a/src/restart.rs"
            path.parent.mkdir(parents=True)
            path.write_text(base + "new_behavior();\n", encoding="utf-8")
            filtered, count = coverage_diff.exclude_unchanged_moved_blocks(
                {"crates/a/src/restart.rs": set(range(1, 11))},
                [("crates/a/src/app.rs", base, "")],
                root,
            )
        self.assertEqual(filtered, {"crates/a/src/restart.rs": {10}})
        self.assertEqual(count, 9)

    def test_copied_block_still_present_in_source_remains_changed(self) -> None:
        base = "\n".join(f"let value_{index} = {index};" for index in range(12))
        moved = coverage_diff.unchanged_moved_block_lines(
            base,
            [("crates/a/src/source.rs", base, base)],
            "crates/a/src/copied.rs",
        )
        self.assertEqual(moved, set())

    def test_retained_middle_block_does_not_create_small_moved_fragments(self) -> None:
        lines = [f"let value_{index} = {index};" for index in range(20)]
        base = "\n".join(lines)
        original_current = "\n".join(lines[6:14])
        moved = coverage_diff.unchanged_moved_block_lines(
            base,
            [("crates/a/src/source.rs", base, original_current)],
            "crates/a/src/moved.rs",
        )
        self.assertEqual(moved, set())

    def test_edited_middle_source_line_does_not_hide_copied_suffix(self) -> None:
        lines = [f"let value_{index} = {index};" for index in range(16)]
        base = "\n".join(lines)
        retained_source = "\n".join(
            [*lines[:8], "let value_8 = changed();", *lines[9:]]
        )
        moved = coverage_diff.unchanged_moved_block_lines(
            base,
            [("crates/a/src/source.rs", base, retained_source)],
            "crates/a/src/copied.rs",
        )
        self.assertEqual(moved, set())

    def test_same_file_copy_keeps_new_duplicate_lines_in_gate(self) -> None:
        base = "\n".join(f"let value_{index} = {index};" for index in range(12))
        for placement in ("before", "after"):
            with self.subTest(placement=placement), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                path = root / "crates/a/src/lib.rs"
                path.parent.mkdir(parents=True)
                current = base + "\n" + base
                path.write_text(current, encoding="utf-8")
                copied_lines = (
                    set(range(1, 13))
                    if placement == "before"
                    else set(range(13, 25))
                )
                filtered, count = coverage_diff.exclude_unchanged_moved_blocks(
                    {"crates/a/src/lib.rs": copied_lines},
                    [("crates/a/src/lib.rs", base, current)],
                    root,
                )
            self.assertEqual(filtered["crates/a/src/lib.rs"], copied_lines)
            self.assertEqual(count, 0)

    def test_same_file_move_remains_excludable(self) -> None:
        block = "\n".join(f"let value_{index} = {index};" for index in range(12))
        separator = "fn separator() {}"
        base = block + "\n" + separator
        current = separator + "\n" + block
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root / "crates/a/src/lib.rs"
            path.parent.mkdir(parents=True)
            path.write_text(current, encoding="utf-8")
            moved_lines = set(range(2, 14))
            filtered, count = coverage_diff.exclude_unchanged_moved_blocks(
                {"crates/a/src/lib.rs": moved_lines},
                [("crates/a/src/lib.rs", base, current)],
                root,
            )
        self.assertEqual(filtered["crates/a/src/lib.rs"], set())
        self.assertEqual(count, 12)


if __name__ == "__main__":
    unittest.main()
